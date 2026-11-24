use crate::contracts::{
    DagSnapshot, EdgeKind, Freshness, GraphEdge, GraphNode, NodeId, NodeKind, StalePropagationReport,
};
use crate::error::{RelayError, RelayResult};
use chrono::{DateTime, Utc};
use petgraph::Direction;
use petgraph::algo::{has_path_connecting, toposort};
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use std::collections::{HashMap, HashSet, VecDeque};

/// In-memory Decision-Evidence DAG backed by `petgraph::StableDiGraph`.
/// Enforces deterministic staleness propagation whenever an upstream Artifact changes.
#[derive(Clone, Debug, Default)]
pub struct DagEngine {
    repo_id: String,
    head_commit_sha: Option<String>,
    graph: StableDiGraph<GraphNode, GraphEdge>,
    id_to_idx: HashMap<NodeId, NodeIndex>,
    edge_id_to_idx: HashMap<String, EdgeIndex>,
}

impl DagEngine {
    pub fn new(repo_id: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
            ..Self::default()
        }
    }

    pub fn set_head_commit(&mut self, commit_sha: impl Into<String>) {
        self.head_commit_sha = Some(commit_sha.into());
    }

    pub fn upsert_node(&mut self, node: GraphNode) -> NodeIndex {
        if let Some(&idx) = self.id_to_idx.get(&node.id) {
            self.graph[idx] = node;
            idx
        } else {
            let id = node.id.clone();
            let idx = self.graph.add_node(node);
            self.id_to_idx.insert(id, idx);
            idx
        }
    }

    pub fn add_edge(&mut self, edge: GraphEdge) -> RelayResult<()> {
        if !self.edge_id_to_idx.contains_key(&edge.id) {
            self.check_connection(&edge)?;
        }
        self.restore_edge(edge)
    }

    pub(crate) fn restore_edge(&mut self, edge: GraphEdge) -> RelayResult<()> {
        self.write_edge(edge, false).map(|_| ())
    }

    pub(crate) fn upsert_edge(&mut self, edge: GraphEdge) -> RelayResult<bool> {
        self.write_edge(edge, true)
    }

    fn check_connection(&self, edge: &GraphEdge) -> RelayResult<()> {
        if let (Some(&from), Some(&to)) = (self.id_to_idx.get(&edge.from), self.id_to_idx.get(&edge.to))
            && has_path_connecting(&self.graph, to, from, None)
        {
            return Err(RelayError::Internal(format!("edge {} would create a DAG cycle", edge.id)));
        }
        Ok(())
    }

    pub(crate) fn validate_edge_changes(&self, previous: &Self) -> RelayResult<()> {
        // Check the final batch, since replacing two edges can temporarily form a cycle.
        if toposort(&self.graph, None).is_ok() {
            return Ok(());
        }
        // Older versions accepted cycles. Allow unrelated edits and exact replay while
        // rejecting each new connection that participates in a cycle.
        for edge in self.graph.edge_weights() {
            let unchanged = previous.edge_id_to_idx.get(&edge.id).is_some_and(|&idx| {
                let old = &previous.graph[idx];
                old.from == edge.from && old.to == edge.to
            });
            if !unchanged {
                self.check_connection(edge)?;
            }
        }
        Ok(())
    }

    fn write_edge(&mut self, edge: GraphEdge, replace: bool) -> RelayResult<bool> {
        let from = *self
            .id_to_idx
            .get(&edge.from)
            .ok_or_else(|| RelayError::NodeNotFound(edge.from.clone()))?;
        let to = *self
            .id_to_idx
            .get(&edge.to)
            .ok_or_else(|| RelayError::NodeNotFound(edge.to.clone()))?;

        if let Some(&idx) = self.edge_id_to_idx.get(&edge.id) {
            if !replace || self.graph[idx] == edge {
                return Ok(false);
            }
            let _ = self.graph.remove_edge(idx);
        }
        let id = edge.id.clone();
        let idx = self.graph.add_edge(from, to, edge);
        self.edge_id_to_idx.insert(id, idx);
        Ok(true)
    }

    pub(crate) fn refresh_head_commit(&mut self) {
        self.head_commit_sha = self
            .graph
            .node_indices()
            .filter_map(|idx| {
                let node = &self.graph[idx];
                let NodeKind::Artifact { git_commit_sha, .. } = &node.payload else {
                    return None;
                };
                Some((self.artifact_rank(idx), git_commit_sha))
            })
            .max()
            .map(|(_, sha)| sha.clone());
    }

    pub(crate) fn head_commit_sha(&self) -> Option<&str> {
        self.head_commit_sha.as_deref()
    }

    fn artifact_rank(&self, idx: NodeIndex) -> (bool, DateTime<Utc>, DateTime<Utc>, &str) {
        let node = &self.graph[idx];
        let superseded = self.graph.edges_directed(idx, Direction::Incoming).any(|edge| {
            edge.weight().kind == EdgeKind::Supersedes
                && matches!(self.graph[edge.source()].payload, NodeKind::Artifact { .. })
        });
        (!superseded, node.updated_at, node.created_at, &node.id)
    }

    pub(crate) fn artifact_id_for_commit(&self, sha: &str) -> Option<&str> {
        self.graph
            .node_indices()
            .filter_map(|idx| {
                let node = &self.graph[idx];
                if !matches!(&node.payload, NodeKind::Artifact { git_commit_sha, .. } if git_commit_sha == sha) {
                    return None;
                }
                Some(self.artifact_rank(idx))
            })
            .max()
            .map(|(_, _, _, id)| id)
    }

    pub(crate) fn artifact_commit(&self, node_id: &str) -> RelayResult<&str> {
        let node = self
            .get_node(node_id)
            .ok_or_else(|| RelayError::NodeNotFound(node_id.into()))?;
        match &node.payload {
            NodeKind::Artifact { git_commit_sha, .. } => Ok(git_commit_sha),
            _ => Err(RelayError::Internal(format!("node {node_id} is not an artifact"))),
        }
    }

    pub fn get_node(&self, node_id: &str) -> Option<&GraphNode> {
        self.id_to_idx.get(node_id).map(|&idx| &self.graph[idx])
    }

    /// Hard Anti-Hallucination Rule:
    /// Whenever an Artifact node is updated or superseded, traverse all downstream edges
    /// deterministically via BFS and mark every reachable Evidence node as `Freshness::Stale`.
    pub fn propagate_staleness(
        &mut self,
        artifact_node_id: &str,
        reason: impl Into<String>,
    ) -> RelayResult<StalePropagationReport> {
        self.invalidate_evidence(artifact_node_id, artifact_node_id, reason.into())
    }

    pub(crate) fn invalidate_evidence(
        &mut self,
        start: &str,
        cause: &str,
        reason: String,
    ) -> RelayResult<StalePropagationReport> {
        let start_idx = *self
            .id_to_idx
            .get(start)
            .ok_or_else(|| RelayError::NodeNotFound(start.into()))?;
        let cause_idx = *self.id_to_idx.get(cause)
            .ok_or_else(|| RelayError::NodeNotFound(cause.into()))?;
        let invalidated_evidence_ids = self.invalidate_from(&[(start_idx, cause_idx, reason.clone())], Some(Utc::now()));
        Ok(StalePropagationReport {
            trigger_node_id: cause.into(),
            invalidated_evidence_ids,
            reason,
        })
    }

    /// Account for evidence or dependencies received after their artifact was superseded.
    pub(crate) fn reconcile_superseded_evidence(&mut self) -> Vec<String> {
        let mut sources: Vec<_> = self.graph.edge_references()
            .filter(|edge| edge.weight().kind == EdgeKind::Supersedes)
            .map(|edge| (
                edge.target(),
                edge.source(),
                format!("Artifact {} superseded by {}", edge.weight().to, edge.weight().from),
            ))
            .collect();
        sources.sort_unstable_by(|a, b| a.2.cmp(&b.2));
        self.invalidate_from(&sources, None)
    }

    fn invalidate_from(
        &mut self,
        sources: &[(NodeIndex, NodeIndex, String)],
        marked_at: Option<DateTime<Utc>>,
    ) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        for (source, (start, _, _)) in sources.iter().enumerate() {
            if visited.insert(*start) {
                queue.push_back((*start, source));
            }
        }
        let mut invalidated_evidence_ids = Vec::new();

        while let Some((idx, source)) = queue.pop_front() {
            let (start, cause, reason) = &sources[source];
            queue.extend(
                self.graph
                    .neighbors(idx)
                    .filter(|neighbor| neighbor != cause && visited.insert(*neighbor))
                    .map(|neighbor| (neighbor, source)),
            );
            let cause_id = &self.graph[*cause].id;
            if idx == *start
                || !matches!(self.graph[idx].payload, NodeKind::Evidence { .. })
                || matches!(self.graph[idx].freshness, Freshness::Stale { .. })
            {
                continue;
            }
            // A reconciliation can run in independent readers. Persisted inputs must
            // yield the same provenance without incorporating the reader's wall clock.
            let timestamp = marked_at.unwrap_or_else(|| self.graph[*cause].updated_at.max(self.graph[idx].updated_at));
            let freshness = Freshness::Stale {
                caused_by: cause_id.clone(),
                reason: reason.clone(),
                marked_at: timestamp,
            };
            let node = &mut self.graph[idx];
            node.freshness = freshness;
            node.updated_at = node.updated_at.max(timestamp);
            invalidated_evidence_ids.push(node.id.clone());
        }

        invalidated_evidence_ids.sort_unstable();
        invalidated_evidence_ids
    }

    /// Guard used by Agent/MCP layers before citing an Evidence node.
    pub fn assert_evidence_fresh(&self, node_id: &str) -> RelayResult<&GraphNode> {
        let node = self
            .get_node(node_id)
            .ok_or_else(|| RelayError::NodeNotFound(node_id.to_string()))?;
        if let Freshness::Stale {
            caused_by, reason, ..
        } = &node.freshness
        {
            return Err(RelayError::StaleEvidenceBlocked {
                node_id: node.id.clone(),
                caused_by: caused_by.clone(),
                reason: reason.clone(),
            });
        }
        Ok(node)
    }

    /// Register a new Artifact node that supersedes a previous Artifact, automatically
    /// propagating STALE to all Evidence nodes produced from the old Artifact.
    pub fn supersede_artifact(
        &mut self,
        old_artifact_id: &str,
        new_artifact: GraphNode,
        reason: impl Into<String>,
    ) -> RelayResult<StalePropagationReport> {
        self.artifact_commit(old_artifact_id)?;
        let new_id = new_artifact.id.clone();
        if new_id == old_artifact_id || !matches!(new_artifact.payload, NodeKind::Artifact { .. }) {
            return Err(RelayError::Internal(format!(
                "superseding node {new_id} must be a distinct artifact",
            )));
        }
        let edge = supersedes_edge(old_artifact_id, &new_id);
        if !self.edge_id_to_idx.get(&edge.id).is_some_and(|&idx| self.graph[idx] == edge) {
            self.check_connection(&edge)?;
        }
        self.upsert_node(new_artifact);
        self.upsert_edge(edge)?;
        self.refresh_head_commit();
        self.invalidate_evidence(old_artifact_id, &new_id, reason.into())
    }

    pub(crate) fn stale_count(&self) -> u32 {
        self.graph
            .node_weights()
            .filter(|node| matches!(node.freshness, Freshness::Stale { .. }))
            .count() as u32
    }

    pub fn snapshot(&self, include_stale: bool) -> DagSnapshot {
        let mut stale_count: u32 = 0;
        let mut nodes = Vec::new();
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            let is_stale = matches!(node.freshness, Freshness::Stale { .. });
            if is_stale {
                stale_count += 1;
            }
            if include_stale || !is_stale {
                nodes.push(node.clone());
            }
        }
        nodes.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        let allowed_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let mut edges: Vec<_> = self
            .graph
            .edge_weights()
            .filter(|e| {
                allowed_ids.contains(e.from.as_str()) && allowed_ids.contains(e.to.as_str())
            })
            .cloned()
            .collect();
        edges.sort_unstable_by(|a, b| a.id.cmp(&b.id));

        DagSnapshot {
            repo_id: self.repo_id.clone(),
            head_commit_sha: self.head_commit_sha.clone(),
            nodes,
            edges,
            stale_count,
            captured_at: Utc::now(),
        }
    }
}

pub(crate) fn supersedes_edge(old_id: &str, new_id: &str) -> GraphEdge {
    GraphEdge {
        id: format!("supersedes:{new_id}->{old_id}"),
        from: new_id.into(),
        to: old_id.into(),
        kind: EdgeKind::Supersedes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_artifact(id: &str, sha: &str) -> GraphNode {
        let now = Utc::now();
        GraphNode {
            id: id.into(),
            label: format!("Artifact {sha}"),
            payload: NodeKind::Artifact {
                git_commit_sha: sha.into(),
                env_fingerprint: "uv-lock-v1".into(),
                working_tree_dirty: false,
                changed_files: vec!["src/forecast.py".into()],
            },
            freshness: Freshness::Fresh,
            device_origin: "macbook-client".into(),
            created_at: now,
            updated_at: now,
        }
    }

    fn sample_evidence(id: &str, task_id: &str) -> GraphNode {
        let now = Utc::now();
        GraphNode {
            id: id.into(),
            label: format!("Evidence {id}"),
            payload: NodeKind::Evidence {
                task_id: task_id.into(),
                preset: "train_grid".into(),
                metrics_json: json!({"mae": 0.142, "window": 14}).into(),
                log_blob_hash: "blake3:deadbeef".into(),
            },
            freshness: Freshness::Fresh,
            device_origin: "windows-host".into(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn upstream_artifact_supersede_marks_downstream_evidence_stale() {
        let mut dag = DagEngine::new("forecast-bench");
        dag.upsert_node(sample_artifact("art-1", "aaa111"));
        dag.upsert_node(sample_evidence("ev-1", "task-1"));
        dag.add_edge(GraphEdge {
            id: "e1".into(),
            from: "art-1".into(),
            to: "ev-1".into(),
            kind: EdgeKind::ProducedFrom,
        })
        .unwrap();

        assert!(dag.assert_evidence_fresh("ev-1").is_ok());

        let report = dag
            .supersede_artifact(
                "art-1",
                sample_artifact("art-2", "bbb222"),
                "src/forecast.py modified on laptop",
            )
            .unwrap();

        assert_eq!(report.trigger_node_id, "art-2");
        assert_eq!(report.invalidated_evidence_ids, vec!["ev-1".to_string()]);
        assert!(matches!(
            dag.assert_evidence_fresh("ev-1"),
            Err(RelayError::StaleEvidenceBlocked { .. })
        ));

        let snap = dag.snapshot(true);
        assert_eq!(snap.stale_count, 1);
        assert_eq!(snap.head_commit_sha.as_deref(), Some("bbb222"));

        let fresh_only = dag.snapshot(false);
        assert_eq!(fresh_only.nodes.len(), 2);
        assert_eq!(fresh_only.stale_count, 1);
    }
}
