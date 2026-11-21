use crate::contracts::{
    ApprovalDecision, DagDelta, DagSnapshot, EdgeKind, Freshness, GraphEdge, GraphNode,
    HandoffSummary, NodeKind, PatchProposal, RuntimeMode, StalePropagationReport, TaskId,
    TaskRecord, TaskSpec, TaskState,
};
use crate::dag::{DagEngine, supersedes_edge};
use crate::error::{RelayError, RelayResult};
use crate::state::{is_active, validate_transition};
use crate::storage::{
    RelayStore, fetch_task, fetch_tasks, insert_task, write_edge, write_freshness, write_handoff,
    write_log_blob, write_node, write_task,
};
use chrono::Utc;
use serde_json::Value;
use sqlx::{Sqlite, SqliteConnection, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

/// Coordinates Relay state for one repository.
#[derive(Clone, Debug)]
pub struct RelayEngine {
    repo_id: String,
    store: RelayStore,
    dag: Arc<RwLock<DagEngine>>,
    mode: Arc<RwLock<RuntimeMode>>,
    repo_root: Option<PathBuf>,
}

impl RelayEngine {
    pub async fn open(
        repo_id: impl Into<String>,
        db_path: impl AsRef<Path>,
        repo_root: Option<PathBuf>,
    ) -> RelayResult<Self> {
        let store = RelayStore::connect(db_path.as_ref()).await?;
        Self::load(repo_id.into(), store, repo_root).await
    }

    pub async fn in_memory(
        repo_id: impl Into<String>,
        repo_root: Option<PathBuf>,
    ) -> RelayResult<Self> {
        let store = RelayStore::connect_in_memory().await?;
        Self::load(repo_id.into(), store, repo_root).await
    }

    async fn load(repo_id: String, store: RelayStore, repo_root: Option<PathBuf>) -> RelayResult<Self> {
        let (nodes, edges) = store.load_dag(&repo_id).await?;
        let mut dag = DagEngine::new(&repo_id);
        for node in nodes {
            dag.upsert_node(node);
        }
        for edge in edges {
            dag.add_edge(edge)?;
        }
        dag.refresh_head_commit();
        let mode = store
            .get_runtime_mode(&repo_id)
            .await?
            .unwrap_or(RuntimeMode::Host);
        Ok(Self {
            repo_id,
            store,
            dag: Arc::new(RwLock::new(dag)),
            mode: Arc::new(RwLock::new(mode)),
            repo_root,
        })
    }

    pub fn repo_root(&self) -> Option<&Path> {
        self.repo_root.as_deref()
    }

    pub async fn dag_snapshot(&self, include_stale: bool) -> DagSnapshot {
        self.dag.read().await.snapshot(include_stale)
    }

    pub async fn upsert_node_and_edges(
        &self,
        node: GraphNode,
        edges: Vec<GraphEdge>,
    ) -> RelayResult<DagSnapshot> {
        self.upsert_graph(vec![node], edges, false).await
    }

    pub async fn apply_dag_delta(&self, delta: DagDelta) -> RelayResult<DagSnapshot> {
        self.check_repo(&delta.repo_id)?;
        self.upsert_graph(delta.upsert_nodes, delta.upsert_edges, true).await
    }

    async fn upsert_graph(
        &self,
        nodes: Vec<GraphNode>,
        edges: Vec<GraphEdge>,
        from_peer: bool,
    ) -> RelayResult<DagSnapshot> {
        let dag = self.dag.clone().write_owned().await;
        let mut staged = dag.clone();
        let mut changed_nodes = BTreeSet::new();
        let mut updated_artifacts = BTreeSet::new();
        let mut stale_commits = BTreeMap::new();

        for mut node in nodes {
            if let Some(existing) = staged.get_node(&node.id) {
                if from_peer && node.updated_at < existing.updated_at {
                    if matches!(node.freshness, Freshness::Stale { .. })
                        && matches!(existing.payload, NodeKind::Evidence { .. })
                        && !matches!(existing.freshness, Freshness::Stale { .. })
                    {
                        // A peer can invalidate evidence even when its clock is behind ours.
                        let freshness = node.freshness;
                        node = existing.clone();
                        node.freshness = freshness;
                    } else {
                        continue;
                    }
                }
                if matches!(node.payload, NodeKind::Evidence { .. })
                    && matches!(existing.freshness, Freshness::Stale { .. })
                    && !matches!(node.freshness, Freshness::Stale { .. })
                {
                    node.freshness = existing.freshness.clone();
                    node.updated_at = node.updated_at.max(existing.updated_at);
                }
                if let NodeKind::Artifact {
                    git_commit_sha: old_sha,
                    ..
                } = &existing.payload
                    && let NodeKind::Artifact {
                        git_commit_sha: new_sha,
                        ..
                    } = &node.payload
                    && node.payload != existing.payload
                {
                    updated_artifacts.insert(node.id.clone());
                    if old_sha != new_sha {
                        stale_commits
                            .insert(old_sha.clone(), format!("Artifact {} updated", node.id));
                    }
                }
            }
            changed_nodes.insert(node.id.clone());
            staged.upsert_node(node);
        }

        let mut supersessions = BTreeMap::new();
        for edge in &edges {
            staged.upsert_edge(edge.clone())?;
            if edge.kind == EdgeKind::Supersedes {
                supersessions.insert(edge.id.as_str(), edge);
            } else {
                supersessions.remove(edge.id.as_str());
            }
        }
        // Traverse the finished graph so invalidation also covers edges received in this delta.
        let mut invalidated = BTreeSet::new();
        for edge in supersessions.into_values() {
            let old_sha = staged.artifact_commit(&edge.to)?;
            let new_sha = staged.artifact_commit(&edge.from)?;
            let reason = format!("Artifact {} superseded by {}", edge.to, edge.from);
            if old_sha != new_sha {
                stale_commits.insert(old_sha.into(), reason.clone());
            }
            let report = staged.invalidate_evidence(&edge.to, &edge.from, reason)?;
            invalidated.extend(report.invalidated_evidence_ids);
        }
        for id in updated_artifacts {
            let reason = format!("Artifact {id} updated");
            let report = staged.propagate_staleness(&id, reason)?;
            invalidated.extend(report.invalidated_evidence_ids);
        }
        staged.refresh_head_commit();

        let mut tx = self.store.begin_write().await?;
        for id in &changed_nodes {
            if let Some(node) = staged.get_node(id) {
                write_node(&mut tx, &self.repo_id, node).await?;
            }
        }
        for edge in &edges {
            write_edge(&mut tx, &self.repo_id, edge).await?;
        }
        write_freshness(
            &mut tx,
            invalidated.difference(&changed_nodes).filter_map(|id| staged.get_node(id)),
        )
        .await?;
        self.stale_tasks(&mut tx, &stale_commits).await?;
        let snapshot = staged.snapshot(true);
        commit_dag(tx, dag, staged).await?;
        Ok(snapshot)
    }

    pub async fn supersede_artifact(
        &self,
        old_artifact_id: &str,
        new_artifact: GraphNode,
        reason: impl Into<String>,
    ) -> RelayResult<StalePropagationReport> {
        let dag = self.dag.clone().write_owned().await;
        let old_sha = dag.artifact_commit(old_artifact_id)?.to_owned();
        let edge = supersedes_edge(old_artifact_id, &new_artifact.id);
        let mut staged = dag.clone();
        let report = staged.supersede_artifact(old_artifact_id, new_artifact, reason)?;
        let new_node = staged
            .get_node(&edge.from)
            .ok_or_else(|| RelayError::NodeNotFound(edge.from.clone()))?;

        let mut tx = self.store.begin_write().await?;
        write_node(&mut tx, &self.repo_id, new_node).await?;
        write_edge(&mut tx, &self.repo_id, &edge).await?;
        write_freshness(
            &mut tx,
            report
                .invalidated_evidence_ids
                .iter()
                .filter_map(|id| staged.get_node(id)),
        )
        .await?;
        if old_sha != staged.artifact_commit(&edge.from)? {
            self.stale_tasks(&mut tx, &BTreeMap::from([(old_sha, report.reason.clone())]))
                .await?;
        }
        commit_dag(tx, dag, staged).await?;
        Ok(report)
    }

    pub async fn read_evidence_log(
        &self,
        node_id: &str,
        max_lines: u32,
    ) -> RelayResult<(GraphNode, String)> {
        let dag = self.dag.read().await;
        let node = dag.assert_evidence_fresh(node_id)?;
        let NodeKind::Evidence { log_blob_hash, .. } = &node.payload else {
            return Err(RelayError::Internal(format!("node {node_id} is not evidence")));
        };

        let content = self
            .store
            .get_log_blob(log_blob_hash)
            .await?
            .ok_or_else(|| {
                RelayError::Internal(format!(
                    "log blob {log_blob_hash} for evidence {node_id} is missing",
                ))
            })?;
        let mut tail: Vec<_> = content.lines().rev().take(max_lines as usize).collect();
        tail.reverse();
        Ok((node.clone(), tail.join("\n")))
    }

    pub async fn submit_task(&self, mut spec: TaskSpec) -> RelayResult<(TaskRecord, bool)> {
        self.check_repo(&spec.repo_id)?;
        if spec.task_id.is_nil() {
            spec.task_id = Uuid::now_v7();
        }
        let now = Utc::now();
        let queued = TaskRecord {
            spec,
            state: TaskState::Queued,
            active_proposal: None,
            preflight_summary: None,
            evidence_node_id: None,
            error_message: None,
            created_at: now,
            updated_at: now,
        };

        let dag = self.dag.read().await;
        let mut tx = self.store.begin_write().await?;
        let (record, inserted) = insert_task(&mut tx, &queued).await?;
        self.check_repo(&record.spec.repo_id)?;
        if inserted {
            let tasks = fetch_tasks(&mut tx, Some(&self.repo_id)).await?;
            let handoff = HandoffSummary {
                handoff_id: record.spec.task_id.to_string(),
                repo_id: self.repo_id.clone(),
                source_device: record.spec.requested_by_device.clone(),
                base_commit_sha: record.spec.base_commit_sha.clone(),
                summary: record
                    .spec
                    .user_note
                    .clone()
                    .unwrap_or_else(|| format!("Queued {} task", record.spec.command_preset)),
                active_task_count: tasks
                    .iter()
                    .filter(|task| is_active(task.state))
                    .count() as u32,
                stale_evidence_count: dag.stale_count(),
                created_at: record.created_at,
            };
            write_handoff(&mut tx, &handoff).await?;
        }
        tx.commit().await?;
        Ok((record, inserted))
    }

    pub async fn transition_task(
        &self,
        task_id: TaskId,
        next_state: TaskState,
        summary: Option<String>,
        error: Option<String>,
    ) -> RelayResult<TaskRecord> {
        self.edit_task(task_id, |record| {
            move_task(record, next_state)?;
            if let Some(summary) = summary {
                record.preflight_summary = Some(summary);
            }
            record.error_message = error;
            Ok(())
        })
        .await
    }

    pub async fn attach_patch_proposal(&self, proposal: PatchProposal) -> RelayResult<TaskRecord> {
        self.edit_task(proposal.task_id, |record| {
            move_task(record, TaskState::SelfHealingProposed)?;
            record.active_proposal = Some(proposal);
            Ok(())
        })
        .await
    }

    pub async fn resolve_approval(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
    ) -> RelayResult<TaskRecord> {
        self.edit_task(task_id, |record| {
            if let Some(patch_id) = patch_id
                && record
                    .active_proposal
                    .as_ref()
                    .map(|patch| patch.patch_id.as_str())
                    != Some(patch_id)
            {
                return Err(RelayError::ProposalNotFound(patch_id.into()));
            }
            let next_state = match decision {
                ApprovalDecision::Approved => TaskState::Executing,
                ApprovalDecision::Rejected => TaskState::Rejected,
            };
            move_task(record, next_state)
        })
        .await
    }

    pub async fn record_evidence_result(
        &self,
        task_id: TaskId,
        artifact_node_id: &str,
        mut evidence_node: GraphNode,
        raw_log: &str,
    ) -> RelayResult<TaskRecord> {
        let hash = blake3::hash(raw_log.as_bytes()).to_hex().to_string();
        let NodeKind::Evidence {
            task_id: evidence_task_id,
            log_blob_hash,
            ..
        } = &mut evidence_node.payload
        else {
            return Err(RelayError::Internal(format!(
                "node {} is not evidence",
                evidence_node.id,
            )));
        };
        let task_id_text = task_id.to_string();
        evidence_task_id.clone_from(&task_id_text);
        log_blob_hash.clone_from(&hash);

        let dag = self.dag.clone().write_owned().await;
        dag.artifact_commit(artifact_node_id)?;
        let edge = GraphEdge {
            id: format!("produced_from:{artifact_node_id}->{}", evidence_node.id),
            from: artifact_node_id.into(),
            to: evidence_node.id.clone(),
            kind: EdgeKind::ProducedFrom,
        };
        let mut tx = self.store.begin_write().await?;
        let mut task = self.require_task(&mut tx, task_id).await?;
        move_task(&mut task, TaskState::Completed)?;
        if let Some(existing) = dag.get_node(&evidence_node.id) {
            match &existing.payload {
                NodeKind::Evidence { task_id, .. } if task_id == &task_id_text => {
                    if matches!(existing.freshness, Freshness::Stale { .. }) {
                        evidence_node.freshness = existing.freshness.clone();
                        evidence_node.updated_at = evidence_node.updated_at.max(existing.updated_at);
                    }
                }
                _ => {
                    return Err(RelayError::Internal(format!(
                        "node {} already belongs to a different result",
                        evidence_node.id,
                    )));
                }
            }
        }

        let mut staged = dag.clone();
        staged.upsert_node(evidence_node.clone());
        staged.upsert_edge(edge.clone())?;
        task.evidence_node_id = Some(evidence_node.id.clone());
        task.error_message = None;

        write_log_blob(&mut tx, &hash, raw_log).await?;
        write_node(&mut tx, &self.repo_id, &evidence_node).await?;
        write_edge(&mut tx, &self.repo_id, &edge).await?;
        write_task(&mut tx, &task).await?;
        commit_dag(tx, dag, staged).await?;
        Ok(task)
    }

    pub async fn list_tasks(&self) -> RelayResult<Vec<TaskRecord>> {
        self.store.list_tasks(Some(&self.repo_id)).await
    }

    pub async fn get_task(&self, task_id: TaskId) -> RelayResult<Option<TaskRecord>> {
        Ok(self
            .store
            .get_task(task_id)
            .await?
            .filter(|task| task.spec.repo_id == self.repo_id))
    }

    pub async fn list_handoffs(&self, limit: u32) -> RelayResult<Vec<HandoffSummary>> {
        self.store.list_repo_handoffs(Some(&self.repo_id), limit).await
    }

    pub async fn enqueue_outbox(&self, payload: &Value) -> RelayResult<i64> {
        self.store.enqueue_outbox(&self.repo_id, payload).await
    }

    pub async fn pending_outbox_count(&self) -> RelayResult<u32> {
        self.store.count_repo_pending_outbox(Some(&self.repo_id)).await
    }

    pub async fn mark_outbox_delivered(&self, seq: i64) -> RelayResult<()> {
        self.store.mark_repo_outbox_delivered(Some(&self.repo_id), seq).await
    }

    pub async fn runtime_mode(&self) -> RuntimeMode {
        *self.mode.read().await
    }

    pub async fn set_runtime_mode(&self, mode: RuntimeMode) -> RelayResult<()> {
        let mut current = self.mode.clone().write_owned().await;
        let repo_id = self.repo_id.clone();
        let store = self.store.clone();
        tokio::spawn(async move {
            store.put_runtime_mode(&repo_id, mode).await?;
            *current = mode;
            Ok(())
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    async fn require_task(
        &self,
        conn: &mut SqliteConnection,
        task_id: TaskId,
    ) -> RelayResult<TaskRecord> {
        fetch_task(conn, task_id)
            .await?
            .filter(|task| task.spec.repo_id == self.repo_id)
            .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))
    }

    async fn edit_task(
        &self,
        task_id: TaskId,
        update: impl FnOnce(&mut TaskRecord) -> RelayResult<()>,
    ) -> RelayResult<TaskRecord> {
        let mut tx = self.store.begin_write().await?;
        let mut task = self.require_task(&mut tx, task_id).await?;
        update(&mut task)?;
        write_task(&mut tx, &task).await?;
        tx.commit().await?;
        Ok(task)
    }

    async fn stale_tasks(
        &self,
        conn: &mut SqliteConnection,
        stale_commits: &BTreeMap<String, String>,
    ) -> RelayResult<()> {
        if stale_commits.is_empty() {
            return Ok(());
        }
        for mut task in fetch_tasks(conn, Some(&self.repo_id)).await? {
            if let Some(reason) = stale_commits.get(&task.spec.base_commit_sha)
                && is_active(task.state)
            {
                move_task(&mut task, TaskState::Stale)?;
                task.error_message = Some(reason.clone());
                write_task(conn, &task).await?;
            }
        }
        Ok(())
    }

    fn check_repo(&self, repo_id: &str) -> RelayResult<()> {
        if self.repo_id != repo_id {
            return Err(RelayError::Internal(format!(
                "repository {repo_id} does not match engine repository {}",
                self.repo_id,
            )));
        }
        Ok(())
    }
}

fn move_task(task: &mut TaskRecord, next_state: TaskState) -> RelayResult<()> {
    validate_transition(task.spec.task_id, task.state, next_state)?;
    task.state = next_state;
    task.updated_at = task.updated_at.max(Utc::now());
    Ok(())
}

// Keep the write guard through commit and publication, even if the caller stops waiting.
async fn commit_dag(
    tx: Transaction<'static, Sqlite>,
    mut dag: OwnedRwLockWriteGuard<DagEngine>,
    staged: DagEngine,
) -> RelayResult<()> {
    tokio::spawn(async move {
        tx.commit().await?;
        *dag = staged;
        Ok(())
    })
    .await
    .map_err(|err| RelayError::Internal(err.to_string()))?
}

#[cfg(test)]
mod tests;
