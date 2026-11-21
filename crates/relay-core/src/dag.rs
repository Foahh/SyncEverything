use crate::contracts::{
    DagSnapshot, EdgeKind, Freshness, GraphEdge, GraphNode, NodeId, NodeKind,
    StalePropagationReport,
};
use crate::error::{RelayError, RelayResult};
use chrono::Utc;
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Bfs;
use std::collections::HashMap;

/// In-memory Decision-Evidence DAG backed by `petgraph::DiGraph`.
/// Enforces deterministic staleness propagation whenever an upstream Artifact changes.
#[derive(Debug, Default)]
pub struct DagEngine {
    repo_id: String,
    head_commit_sha: Option<String>,
    graph: DiGraph<GraphNode, GraphEdge>,
    id_to_idx: HashMap<NodeId, NodeIndex>,
}

impl DagEngine {
    pub fn new(repo_id: impl Into<String>) -> Self {
        Self {
            repo_id: repo_id.into(),
            head_commit_sha: None,
            graph: DiGraph::new(),
            id_to_idx: HashMap::new(),
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
        let from_idx = *self
            .id_to_idx
            .get(&edge.from)
            .ok_or_else(|| RelayError::NodeNotFound(edge.from.clone()))?;
        let to_idx = *self
            .id_to_idx
            .get(&edge.to)
            .ok_or_else(|| RelayError::NodeNotFound(edge.to.clone()))?;

        // Prevent duplicate edge IDs
        let exists = self
            .graph
            .edges_directed(from_idx, Direction::Outgoing)
            .any(|e| e.weight().id == edge.id);
        if !exists {
            self.graph.add_edge(from_idx, to_idx, edge);
        }
        Ok(())
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
        let start_idx = *self
            .id_to_idx
            .get(artifact_node_id)
            .ok_or_else(|| RelayError::NodeNotFound(artifact_node_id.to_string()))?;

        let reason_str = reason.into();
        let now = Utc::now();
        let mut invalidated_evidence_ids = Vec::new();
        let mut to_mark: Vec<NodeIndex> = Vec::new();

        let mut bfs = Bfs::new(&self.graph, start_idx);
        while let Some(nx) = bfs.next(&self.graph) {
            if nx == start_idx {
                continue;
            }
            if matches!(self.graph[nx].payload, NodeKind::Evidence { .. }) {
                to_mark.push(nx);
            }
        }

        for nx in to_mark {
            let node = &mut self.graph[nx];
            node.freshness = Freshness::Stale {
                caused_by: artifact_node_id.to_string(),
                reason: reason_str.clone(),
                marked_at: now,
            };
            node.updated_at = now;
            invalidated_evidence_ids.push(node.id.clone());
        }

        Ok(StalePropagationReport {
            trigger_node_id: artifact_node_id.to_string(),
            invalidated_evidence_ids,
            reason: reason_str,
        })
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
        if !self.id_to_idx.contains_key(old_artifact_id) {
            return Err(RelayError::NodeNotFound(old_artifact_id.to_string()));
        }
        let new_id = new_artifact.id.clone();
        if let NodeKind::Artifact {
            ref git_commit_sha, ..
        } = new_artifact.payload
        {
            self.head_commit_sha = Some(git_commit_sha.clone());
        }
        self.upsert_node(new_artifact);
        self.add_edge(GraphEdge {
            id: format!("supersedes:{new_id}->{old_artifact_id}"),
            from: new_id.clone(),
            to: old_artifact_id.to_string(),
            kind: EdgeKind::Supersedes,
        })?;
        self.propagate_staleness(&new_id, reason)
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
        let allowed_ids: HashMap<&str, ()> = nodes.iter().map(|n| (n.id.as_str(), ())).collect();
        let edges = self
            .graph
            .edge_weights()
            .filter(|e| {
                allowed_ids.contains_key(e.from.as_str()) && allowed_ids.contains_key(e.to.as_str())
            })
            .cloned()
            .collect();

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
