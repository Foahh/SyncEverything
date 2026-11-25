use crate::agent::AgentChatMessage;
use crate::contracts::{
    ApprovalDecision, DagDelta, DagSnapshot, EdgeKind, Freshness, GraphEdge, GraphNode,
    HandoffSummary, NodeKind, PatchProposal, RuntimeMode, StalePropagationReport, TaskId,
    TaskLogChunk, TaskRecord, TaskSpec, TaskState, TrustedPeerRecord, WireRequest,
};
use crate::dag::{DagEngine, supersedes_edge};
use crate::error::{RelayError, RelayResult};
use crate::sandbox::{HeadSyncOutcome, PresetRunOptions};
use crate::state::{is_active, validate_transition};
use crate::storage::{
    OutboxSnapshot, RelayStore, fetch_task, fetch_task_log_ref, fetch_tasks, insert_task,
    write_edge, write_freshness, write_handoff, write_log_blob, write_node, write_outbox,
    write_task, write_task_log_ref,
};
use chrono::Utc;
use serde_json::Value;
use sqlx::{Sqlite, SqliteConnection, Transaction};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard, broadcast, oneshot, watch};
use uuid::Uuid;

const TASK_LOG_BROADCAST_CAPACITY: usize = 1024;
const CROSS_PROCESS_CANCEL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct TaskCancellationToken {
    rx: watch::Receiver<bool>,
}

impl TaskCancellationToken {
    pub fn new() -> (watch::Sender<bool>, Self) {
        let (tx, rx) = watch::channel(false);
        (tx, Self { rx })
    }

    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    pub async fn cancelled(&mut self) {
        if self.rx.wait_for(|&cancelled| cancelled).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Debug)]
struct CancellationEntry {
    id: u64,
    tx: watch::Sender<bool>,
}

/// RAII handle that removes a task's active cancellation signal from `RelayEngine` on drop.
#[derive(Debug)]
pub struct TaskCancellationGuard {
    registry: Arc<StdRwLock<HashMap<TaskId, CancellationEntry>>>,
    task_id: TaskId,
    entry_id: u64,
    token: TaskCancellationToken,
    _stop_observer: Option<oneshot::Sender<()>>,
}

impl TaskCancellationGuard {
    pub fn token(&self) -> TaskCancellationToken {
        self.token.clone()
    }
}

impl Drop for TaskCancellationGuard {
    fn drop(&mut self) {
        if let Ok(mut handles) = self.registry.write()
            && handles
                .get(&self.task_id)
                .is_some_and(|entry| entry.id == self.entry_id)
        {
            handles.remove(&self.task_id);
        }
    }
}

enum EvidenceArtifact<'a> {
    Existing(&'a str),
    FromCommit(&'a GraphNode),
}

#[derive(Debug)]
struct CachedDag {
    graph: DagEngine,
    revision: i64,
}

async fn dag_revision(conn: &mut SqliteConnection) -> RelayResult<i64> {
    Ok(sqlx::query_scalar("SELECT value FROM dag_revision WHERE id = 1")
        .fetch_one(conn).await?)
}

async fn load_graph(conn: &mut SqliteConnection, repo_id: &str) -> RelayResult<DagEngine> {
    let (nodes, edges) = RelayStore::fetch_dag(conn, repo_id).await?;
    let mut graph = DagEngine::new(repo_id);
    for node in nodes {
        graph.upsert_node(node);
    }
    for edge in edges {
        graph.restore_edge(edge)?;
    }
    graph.refresh_head_commit();
    Ok(graph)
}

/// Coordinates Relay state for one repository.
#[derive(Clone, Debug)]
pub struct RelayEngine {
    repo_id: String,
    store: RelayStore,
    dag: Arc<Mutex<CachedDag>>,
    mode: watch::Sender<RuntimeMode>,
    outbox_counts: Arc<StdRwLock<OutboxSnapshot>>,
    metadata_writes: Arc<Mutex<()>>,
    outbox_flush: Arc<Mutex<()>>,
    cancellation_handles: Arc<StdRwLock<HashMap<TaskId, CancellationEntry>>>,
    next_cancel_entry_id: Arc<AtomicU64>,
    log_tx: broadcast::Sender<TaskLogChunk>,
    log_seqs: Arc<StdRwLock<HashMap<TaskId, Arc<AtomicU32>>>>,
    repo_root: Option<PathBuf>,
}

impl RelayEngine {
    pub async fn open(
        repo_id: impl Into<String>,
        db_path: impl AsRef<Path>,
        repo_root: Option<PathBuf>,
    ) -> RelayResult<Self> {
        Self::open_with_default_mode(repo_id, db_path, repo_root, RuntimeMode::Host).await
    }

    pub async fn open_with_default_mode(
        repo_id: impl Into<String>,
        db_path: impl AsRef<Path>,
        repo_root: Option<PathBuf>,
        default_mode: RuntimeMode,
    ) -> RelayResult<Self> {
        let store = RelayStore::connect(db_path.as_ref()).await?;
        Self::load(repo_id.into(), store, repo_root, default_mode).await
    }

    pub async fn in_memory(
        repo_id: impl Into<String>,
        repo_root: Option<PathBuf>,
    ) -> RelayResult<Self> {
        let store = RelayStore::connect_in_memory().await?;
        Self::load(repo_id.into(), store, repo_root, RuntimeMode::Host).await
    }

    async fn load(
        repo_id: String,
        store: RelayStore,
        repo_root: Option<PathBuf>,
        default_mode: RuntimeMode,
    ) -> RelayResult<Self> {
        let mut tx = store.begin_write().await?;
        let mut graph = load_graph(&mut tx, &repo_id).await?;
        let repaired = graph.reconcile_superseded_evidence();
        let nodes: Vec<_> = repaired.iter().filter_map(|id| graph.get_node(id)).collect();
        write_freshness(&mut tx, nodes).await?;
        let revision = dag_revision(&mut tx).await?;
        tx.commit().await?;
        let mode = store
            .get_runtime_mode(&repo_id)
            .await?
            .unwrap_or(default_mode);
        let outbox_counts = store.load_outbox_snapshot(&repo_id).await?;
        let (log_tx, _) = broadcast::channel(TASK_LOG_BROADCAST_CAPACITY);
        Ok(Self {
            repo_id,
            store,
            dag: Arc::new(Mutex::new(CachedDag { graph, revision })),
            mode: watch::Sender::new(mode),
            outbox_counts: Arc::new(StdRwLock::new(outbox_counts)),
            metadata_writes: Arc::new(Mutex::new(())),
            outbox_flush: Arc::new(Mutex::new(())),
            cancellation_handles: Arc::new(StdRwLock::new(HashMap::new())),
            next_cancel_entry_id: Arc::new(AtomicU64::new(1)),
            log_tx,
            log_seqs: Arc::new(StdRwLock::new(HashMap::new())),
            repo_root,
        })
    }

    pub fn subscribe_task_logs(&self) -> broadcast::Receiver<TaskLogChunk> {
        self.log_tx.subscribe()
    }

    pub fn publish_task_log(&self, chunk: TaskLogChunk) {
        let _ = self.log_tx.send(chunk);
    }

    pub fn task_log_seq_counter(&self, task_id: TaskId) -> Arc<AtomicU32> {
        let mut seqs = self.log_seqs.write().expect("task log seq lock poisoned");
        seqs.entry(task_id)
            .or_insert_with(|| Arc::new(AtomicU32::new(0)))
            .clone()
    }

    fn clear_task_log_seq(&self, task_id: TaskId) {
        if let Ok(mut seqs) = self.log_seqs.write() {
            seqs.remove(&task_id);
        }
    }

    pub fn preset_run_options(
        &self,
        task_id: TaskId,
        cancel_token: TaskCancellationToken,
    ) -> PresetRunOptions {
        let engine = self.clone();
        PresetRunOptions {
            task_id: Some(task_id),
            on_log_chunk: Some(Arc::new(move |chunk| engine.publish_task_log(chunk))),
            cancel_token: Some(cancel_token),
            seq_counter: Some(self.task_log_seq_counter(task_id)),
        }
    }

    /// Register an RAII cancellation guard for a running task. Returns `TaskCancelled`
    /// immediately if the task was already cancelled before registration.
    pub async fn register_task_cancellation(
        &self,
        task_id: TaskId,
    ) -> RelayResult<TaskCancellationGuard> {
        let entry_id = self.next_cancel_entry_id.fetch_add(1, Ordering::Relaxed);
        let (tx, token) = TaskCancellationToken::new();
        {
            let mut handles = self
                .cancellation_handles
                .write()
                .expect("cancellation registry poisoned");
            if handles.contains_key(&task_id) {
                return Err(RelayError::Internal(format!(
                    "task {task_id} already has an active worktree operation"
                )));
            }
            handles.insert(task_id, CancellationEntry { id: entry_id, tx: tx.clone() });
        }
        let mut guard = TaskCancellationGuard {
            registry: self.cancellation_handles.clone(),
            task_id,
            entry_id,
            token,
            _stop_observer: None,
        };
        let task = self
            .get_task(task_id)
            .await?
            .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))?;
        if task.state == TaskState::Cancelled {
            return Err(RelayError::TaskCancelled(task_id.to_string()));
        }
        // The desktop and a standalone MCP process can work against the same database.
        // Keep the watch channel for immediate local notification and observe persisted
        // cancellation so a request made by the other process can stop the preset too.
        let engine = self.clone();
        let (stop, mut stopped) = oneshot::channel();
        guard._stop_observer = Some(stop);
        tokio::spawn(async move {
            while !*tx.borrow() {
                tokio::select! {
                    _ = &mut stopped => break,
                    () = tokio::time::sleep(CROSS_PROCESS_CANCEL_INTERVAL) => {}
                }
                // Allow this read to release its SQL connection normally if the
                // operation finishes while the query is running.
                if engine
                    .get_task(task_id)
                    .await
                    .is_ok_and(|task| task.is_some_and(|task| task.state == TaskState::Cancelled))
                {
                    let _ = tx.send(true);
                    break;
                }
            }
        });
        Ok(guard)
    }

    pub fn has_active_cancellation(&self, task_id: TaskId) -> bool {
        self.cancellation_handles
            .read()
            .expect("cancellation registry poisoned")
            .contains_key(&task_id)
    }

    pub fn active_cancellation_count(&self) -> usize {
        self.cancellation_handles
            .read()
            .expect("cancellation registry poisoned")
            .len()
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn repo_root(&self) -> Option<&Path> {
        self.repo_root.as_deref()
    }

    async fn current_dag(&self, conn: &mut SqliteConnection) -> RelayResult<OwnedMutexGuard<CachedDag>> {
        let mut cached = self.dag.clone().lock_owned().await;
        let revision = dag_revision(conn).await?;
        if cached.revision != revision {
            let mut graph = load_graph(conn, &self.repo_id).await?;
            graph.reconcile_superseded_evidence();
            *cached = CachedDag { graph, revision };
        }
        Ok(cached)
    }

    async fn read_dag(&self) -> RelayResult<OwnedMutexGuard<CachedDag>> {
        let mut tx = self.store.pool().begin().await?;
        let dag = self.current_dag(&mut tx).await?;
        tx.commit().await?;
        Ok(dag)
    }

    pub async fn dag_snapshot(&self, include_stale: bool) -> RelayResult<DagSnapshot> {
        Ok(self.read_dag().await?.graph.snapshot(include_stale))
    }

    pub(crate) async fn head_commit_sha(&self) -> RelayResult<Option<String>> {
        Ok(self.read_dag().await?.graph.head_commit_sha().map(str::to_owned))
    }

    pub async fn head_artifact_id(&self) -> RelayResult<Option<String>> {
        let dag = self.read_dag().await?;
        Ok(dag.graph.head_commit_sha().and_then(|sha| dag.graph.artifact_id_for_commit(sha)).map(str::to_owned))
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
        let mut tx = self.store.begin_write().await?;
        let dag = self.current_dag(&mut tx).await?;
        let mut staged = dag.graph.clone();
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
        let mut edges_changed = false;
        for edge in &edges {
            edges_changed |= staged.upsert_edge(edge.clone())?;
            if edge.kind == EdgeKind::Supersedes {
                supersessions.insert(edge.id.as_str(), edge);
            } else {
                supersessions.remove(edge.id.as_str());
            }
        }
        if edges_changed {
            staged.validate_acyclic()?;
        }
        // Traverse the finished graph so invalidation also covers edges received in this delta.
        for edge in supersessions.into_values() {
            let old_sha = staged.artifact_commit(&edge.to)?;
            let new_sha = staged.artifact_commit(&edge.from)?;
            let reason = format!("Artifact {} superseded by {}", edge.to, edge.from);
            if old_sha != new_sha {
                stale_commits.insert(old_sha.into(), reason);
            }
        }
        let mut invalidated: BTreeSet<_> = staged.reconcile_superseded_evidence().into_iter().collect();
        for id in updated_artifacts {
            let reason = format!("Artifact {id} updated");
            let report = staged.propagate_staleness(&id, reason)?;
            invalidated.extend(report.invalidated_evidence_ids);
        }
        staged.refresh_head_commit();

        for id in &changed_nodes {
            if let Some(node) = staged.get_node(id) {
                write_node(&mut tx, &self.repo_id, node).await?;
            }
        }
        for edge in &edges {
            write_edge(&mut tx, &self.repo_id, edge).await?;
        }
        let invalidated_nodes: Vec<&GraphNode> = invalidated
            .difference(&changed_nodes)
            .filter_map(|id| staged.get_node(id))
            .collect();
        write_freshness(&mut tx, invalidated_nodes).await?;
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
        let mut tx = self.store.begin_write().await?;
        let dag = self.current_dag(&mut tx).await?;
        self.persist_supersession(tx, dag, old_artifact_id, new_artifact, reason.into()).await
    }

    pub(crate) async fn observed_git_head(&self, checkout: &Path) -> RelayResult<Option<String>> {
        self.store.observed_git_head(&self.repo_id, checkout).await
    }

    pub(crate) async fn observe_git_artifact(
        &self,
        checkout: &Path,
        mut artifact: GraphNode,
        expected_prior: Option<&str>,
        explicit: bool,
    ) -> RelayResult<HeadSyncOutcome> {
        let NodeKind::Artifact { git_commit_sha, .. } = &artifact.payload else {
            return Err(RelayError::Internal("Git HEAD requires an artifact".into()));
        };
        let commit_sha = git_commit_sha.clone();
        let mut tx = self.store.begin_write().await?;
        let prior = RelayStore::fetch_observed_head(&mut tx, &self.repo_id, checkout).await?;
        if prior.as_deref() != expected_prior {
            // Another watcher finished after this observer sampled the physical checkout.
            return if explicit {
                Err(RelayError::Git("another observer updated Git HEAD during sync; retry sync-head".into()))
            } else {
                Ok(HeadSyncOutcome::Unchanged)
            };
        }
        if !explicit && prior.as_deref() == Some(commit_sha.as_str()) {
            return Ok(HeadSyncOutcome::Unchanged);
        }
        let dag = self.current_dag(&mut tx).await?;
        RelayStore::write_observed_head(&mut tx, &self.repo_id, checkout, &commit_sha).await?;
        if let Some(previous_sha) = dag.graph.head_commit_sha() {
            // Pairing may import a newer artifact before this checkout is first observed.
            // Capture its baseline without treating an unchanged working copy as a new edit.
            if previous_sha == commit_sha || (prior.is_none() && !explicit) {
                tx.commit().await?;
                return Ok(HeadSyncOutcome::Unchanged);
            }
            let old_id = dag.graph.artifact_id_for_commit(previous_sha)
                .ok_or_else(|| RelayError::Internal("previous HEAD has no artifact".into()))?.to_owned();
            if dag.graph.get_node(&artifact.id).is_some() {
                // Revisiting a commit starts a new snapshot without cycling back to its old results.
                artifact.id = format!("{}:{}", artifact.id, blake3::hash(old_id.as_bytes()).to_hex());
            }
            let short_sha: String = commit_sha.chars().take(7).collect();
            let reason = format!("Git commit {short_sha}: {}", artifact.label);
            let report = self.persist_supersession(tx, dag, &old_id, artifact, reason).await?;
            Ok(HeadSyncOutcome::Superseded { commit_sha, report })
        } else {
            let mut staged = dag.graph.clone();
            write_node(&mut tx, &self.repo_id, &artifact).await?;
            staged.upsert_node(artifact);
            staged.refresh_head_commit();
            commit_dag(tx, dag, staged).await?;
            Ok(HeadSyncOutcome::Seeded { commit_sha })
        }
    }

    async fn persist_supersession(
        &self,
        mut tx: Transaction<'static, Sqlite>,
        dag: OwnedMutexGuard<CachedDag>,
        old_artifact_id: &str,
        new_artifact: GraphNode,
        reason: String,
    ) -> RelayResult<StalePropagationReport> {
        let old_sha = dag.graph.artifact_commit(old_artifact_id)?.to_owned();
        let edge = supersedes_edge(old_artifact_id, &new_artifact.id);
        let mut staged = dag.graph.clone();
        let report = staged.supersede_artifact(old_artifact_id, new_artifact, reason)?;
        let new_node = staged
            .get_node(&edge.from)
            .ok_or_else(|| RelayError::NodeNotFound(edge.from.clone()))?;

        write_node(&mut tx, &self.repo_id, new_node).await?;
        write_edge(&mut tx, &self.repo_id, &edge).await?;
        let invalidated_nodes: Vec<&GraphNode> = report
            .invalidated_evidence_ids
            .iter()
            .filter_map(|id| staged.get_node(id))
            .collect();
        write_freshness(&mut tx, invalidated_nodes).await?;
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
        let mut tx = self.store.pool().begin().await?;
        let dag = self.current_dag(&mut tx).await?;
        let node = dag.graph.assert_evidence_fresh(node_id)?;
        let NodeKind::Evidence { log_blob_hash, .. } = &node.payload else {
            return Err(RelayError::Internal(format!("node {node_id} is not evidence")));
        };

        let content = RelayStore::fetch_log_blob(&mut tx, log_blob_hash)
            .await?
            .ok_or_else(|| RelayError::MissingEvidenceLog {
                node_id: node_id.to_owned(),
                hash: log_blob_hash.clone(),
            })?;
        tx.commit().await?;
        let mut tail: Vec<_> = content.lines().rev().take(max_lines as usize).collect();
        tail.reverse();
        Ok((node.clone(), tail.join("\n")))
    }

    pub(crate) async fn cache_log_blob(&self, hash: &str, log: &str) -> RelayResult<()> {
        self.store.put_log_blob(hash, log).await
    }

    pub async fn submit_task(&self, spec: TaskSpec) -> RelayResult<(TaskRecord, bool)> {
        self.persist_submitted_task(&queued_task(spec), false, None)
            .await
    }

    /// Record an offline submission and its replay request in the same SQLite transaction.
    pub async fn queue_peer_task(&self, spec: TaskSpec, node_id: Option<String>) -> RelayResult<TaskRecord> {
        let record = queued_task(spec);
        let payload = serde_json::to_value(WireRequest::SubmitTask {
            spec: record.spec.clone(),
        })?;
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let (record, inserted) = engine
                .persist_submitted_task(&record, false, Some((&payload, node_id.as_deref())))
                .await?;
            if inserted {
                engine.record_outbox_enqueued(node_id.as_deref());
            }
            Ok(record)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    /// Store the task state returned by the paired host, including its original timestamps.
    pub async fn mirror_peer_task(&self, record: &TaskRecord) -> RelayResult<bool> {
        self.persist_submitted_task(record, true, None)
            .await
            .map(|(_, changed)| changed)
    }

    async fn persist_submitted_task(
        &self,
        incoming: &TaskRecord,
        from_peer: bool,
        outbox: Option<(&Value, Option<&str>)>,
    ) -> RelayResult<(TaskRecord, bool)> {
        self.check_repo(&incoming.spec.repo_id)?;
        let mut tx = self.store.begin_write().await?;
        let (mut record, inserted) = insert_task(&mut tx, incoming).await?;
        self.check_repo(&record.spec.repo_id)?;
        let updated = from_peer && &record != incoming;
        if updated {
            write_task(&mut tx, incoming).await?;
            record = incoming.clone();
        }
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
                stale_evidence_count: self.current_dag(&mut tx).await?.graph.stale_count(),
                created_at: record.created_at,
            };
            write_handoff(&mut tx, &handoff).await?;
            if let Some((payload, node_id)) = outbox {
                write_outbox(&mut tx, &self.repo_id, payload, node_id).await?;
            }
        }
        tx.commit().await?;
        Ok((record, inserted || updated))
    }

    pub async fn cancel_task(&self, task_id: TaskId) -> RelayResult<TaskRecord> {
        self.cancel_task_inner(task_id, None).await.map(|(record, _)| record)
    }

    /// Record an offline cancellation and its replay request in the same SQLite transaction.
    pub async fn queue_peer_cancel(&self, task_id: TaskId, node_id: Option<String>) -> RelayResult<TaskRecord> {
        let payload = serde_json::to_value(WireRequest::CancelTask { task_id })?;
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let (record, queued) = engine.cancel_task_inner(task_id, Some((&payload, node_id.as_deref()))).await?;
            if queued {
                engine.record_outbox_enqueued(node_id.as_deref());
            }
            Ok(record)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    async fn cancel_task_inner(
        &self,
        task_id: TaskId,
        outbox: Option<(&Value, Option<&str>)>,
    ) -> RelayResult<(TaskRecord, bool)> {
        let mut tx = self.store.begin_write().await?;
        let mut record = self.require_task(&mut tx, task_id).await?;
        if record.state == TaskState::Cancelled {
            // A remote caller may retry a cancellation after the original reply was lost.
            return Ok((record, false));
        }
        move_task(&mut record, TaskState::Cancelled)?;
        if record.preflight_summary.is_none() {
            record.preflight_summary = Some("Cancelled by user".into());
        }
        record.error_message = None;
        write_task(&mut tx, &record).await?;
        if let Some((payload, node_id)) = outbox {
            write_outbox(&mut tx, &self.repo_id, payload, node_id).await?;
        }
        tx.commit().await?;

        let sender = self
            .cancellation_handles
            .read()
            .expect("cancellation registry poisoned")
            .get(&task_id)
            .map(|entry| entry.tx.clone());
        if let Some(cancel_tx) = sender {
            let _ = cancel_tx.send(true);
        }
        Ok((record, outbox.is_some()))
    }

    /// Persist partial stdout/stderr captured when a running task is cancelled and ensure
    /// the task is recorded in `TaskState::Cancelled`.
    pub async fn record_cancelled_task_log(
        &self,
        task_id: TaskId,
        raw_log: &str,
    ) -> RelayResult<(TaskRecord, String)> {
        let hash = blake3::hash(raw_log.as_bytes()).to_hex().to_string();
        let mut tx = self.store.begin_write().await?;
        let mut task = self.require_task(&mut tx, task_id).await?;
        write_log_blob(&mut tx, &hash, raw_log).await?;
        write_task_log_ref(&mut tx, &self.repo_id, task_id, &hash).await?;
        if task.state != TaskState::Cancelled {
            move_task(&mut task, TaskState::Cancelled)?;
        }
        task.preflight_summary = Some("Cancelled by user".into());
        task.error_message = None;
        task.updated_at = task.updated_at.max(Utc::now());
        write_task(&mut tx, &task).await?;
        tx.commit().await?;
        self.clear_task_log_seq(task_id);
        Ok((task, hash))
    }

    /// Persist a task's captured log blob and associate it with `task_id` for inspection.
    pub async fn record_task_log(&self, task_id: TaskId, raw_log: &str) -> RelayResult<String> {
        let hash = blake3::hash(raw_log.as_bytes()).to_hex().to_string();
        let mut tx = self.store.begin_write().await?;
        let _ = self.require_task(&mut tx, task_id).await?;
        write_log_blob(&mut tx, &hash, raw_log).await?;
        write_task_log_ref(&mut tx, &self.repo_id, task_id, &hash).await?;
        tx.commit().await?;
        Ok(hash)
    }

    /// Read the stored `(log_blob_hash, content)` for a task, if any log has been persisted.
    pub async fn read_task_log(&self, task_id: TaskId) -> RelayResult<Option<(String, String)>> {
        let mut tx = self.store.pool().begin().await?;
        let Some(hash) = fetch_task_log_ref(&mut tx, &self.repo_id, task_id).await? else {
            tx.commit().await?;
            return Ok(None);
        };
        let content = RelayStore::fetch_log_blob(&mut tx, &hash).await?;
        tx.commit().await?;
        Ok(content.map(|text| (hash, text)))
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

    pub(crate) async fn set_preflight_summary(
        &self,
        task_id: TaskId,
        summary: String,
    ) -> RelayResult<TaskRecord> {
        self.edit_task(task_id, |record| {
            if record.state != TaskState::PreflightRunning {
                return Err(RelayError::InvalidStateTransition {
                    task_id: task_id.to_string(),
                    from: format!("{:?}", record.state),
                    to: format!("{:?}", TaskState::PreflightRunning),
                });
            }
            record.preflight_summary = Some(summary);
            record.updated_at = record.updated_at.max(Utc::now());
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
            validate_approval_patch(record, patch_id, false)?;
            move_task(record, approval_state(decision))
        })
        .await
    }

    /// Resolve the exact proposal observed by the UI, including approval without a proposal.
    pub async fn resolve_ui_approval(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
    ) -> RelayResult<TaskRecord> {
        self.resolve_ui_approval_inner(task_id, patch_id, decision, None, false)
            .await
            .map(|(record, _)| record)
    }

    /// Accept a previously applied decision when an outbox sender retries it.
    pub(crate) async fn resolve_peer_approval(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
    ) -> RelayResult<(TaskRecord, bool)> {
        self.resolve_ui_approval_inner(task_id, patch_id, decision, None, true)
            .await
    }

    /// Queue the Client's decision after any pending submission for the same task.
    pub async fn queue_peer_approval(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
        node_id: Option<String>,
    ) -> RelayResult<TaskRecord> {
        let patch_id = patch_id.map(str::to_owned);
        let payload = serde_json::to_value(WireRequest::ResolveApproval {
            task_id,
            patch_id: patch_id.clone(),
            decision,
            comment: None,
        })?;
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let (record, _) = engine
                .resolve_ui_approval_inner(task_id, patch_id.as_deref(), decision, Some((&payload, node_id.as_deref())), false)
                .await?;
            engine.record_outbox_enqueued(node_id.as_deref());
            Ok(record)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    async fn resolve_ui_approval_inner(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
        outbox: Option<(&Value, Option<&str>)>,
        allow_replay: bool,
    ) -> RelayResult<(TaskRecord, bool)> {
        let mut tx = self.store.begin_write().await?;
        let mut record = self.require_task(&mut tx, task_id).await?;
        validate_approval_patch(&record, patch_id, true)?;
        if allow_replay
            && matches!(
                (decision, record.state),
                (ApprovalDecision::Approved, TaskState::Executing | TaskState::Completed | TaskState::Failed)
                    | (ApprovalDecision::Rejected, TaskState::Rejected)
            )
        {
            return Ok((record, false));
        }
        move_task(&mut record, approval_state(decision))?;
        write_task(&mut tx, &record).await?;
        if let Some((payload, node_id)) = outbox {
            write_outbox(&mut tx, &self.repo_id, payload, node_id).await?;
        }
        tx.commit().await?;
        Ok((record, true))
    }

    pub async fn record_evidence_result(
        &self,
        task_id: TaskId,
        artifact_node_id: &str,
        evidence_node: GraphNode,
        raw_log: &str,
    ) -> RelayResult<TaskRecord> {
        self.record_evidence(
            task_id,
            EvidenceArtifact::Existing(artifact_node_id),
            evidence_node,
            raw_log,
        )
        .await
    }

    pub(crate) async fn record_worktree_evidence(
        &self,
        task_id: TaskId,
        base_artifact: &GraphNode,
        evidence_node: GraphNode,
        raw_log: &str,
    ) -> RelayResult<TaskRecord> {
        self.record_evidence(
            task_id,
            EvidenceArtifact::FromCommit(base_artifact),
            evidence_node,
            raw_log,
        )
        .await
    }

    async fn record_evidence(
        &self,
        task_id: TaskId,
        artifact: EvidenceArtifact<'_>,
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

        let mut tx = self.store.begin_write().await?;
        let dag = self.current_dag(&mut tx).await?;
        let (artifact_node_id, new_artifact) = match artifact {
            EvidenceArtifact::Existing(id) => {
                dag.graph.artifact_commit(id)?;
                (id, None)
            }
            EvidenceArtifact::FromCommit(node) => {
                let NodeKind::Artifact { git_commit_sha, .. } = &node.payload else {
                    return Err(RelayError::Internal(format!(
                        "node {} is not an artifact",
                        node.id,
                    )));
                };
                match dag.graph.artifact_id_for_commit(git_commit_sha) {
                    Some(id) => (id, None),
                    None => (node.id.as_str(), Some(node)),
                }
            }
        };
        let edge = GraphEdge {
            id: format!("produced_from:{artifact_node_id}->{}", evidence_node.id),
            from: artifact_node_id.into(),
            to: evidence_node.id.clone(),
            kind: EdgeKind::ProducedFrom,
        };
        let mut task = self.require_task(&mut tx, task_id).await?;
        move_task(&mut task, TaskState::Completed)?;
        if let Some(existing) = dag.graph.get_node(&evidence_node.id) {
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

        let mut staged = dag.graph.clone();
        if let Some(artifact) = new_artifact {
            staged.upsert_node(artifact.clone());
            write_node(&mut tx, &self.repo_id, artifact).await?;
            if let Some(head_id) = dag.graph.head_commit_sha().and_then(|sha| dag.graph.artifact_id_for_commit(sha)) {
                // Executing an explicitly selected revision does not switch the active checkout.
                let historical = supersedes_edge(&artifact.id, head_id);
                staged.upsert_edge(historical.clone())?;
                write_edge(&mut tx, &self.repo_id, &historical).await?;
            }
            staged.refresh_head_commit();
        }
        staged.upsert_node(evidence_node.clone());
        staged.upsert_edge(edge.clone())?;
        staged.validate_acyclic()?;
        let invalidated = staged.reconcile_superseded_evidence();
        task.evidence_node_id = Some(evidence_node.id.clone());
        task.error_message = None;

        write_log_blob(&mut tx, &hash, raw_log).await?;
        write_task_log_ref(&mut tx, &self.repo_id, task_id, &hash).await?;
        write_node(&mut tx, &self.repo_id, &evidence_node).await?;
        write_edge(&mut tx, &self.repo_id, &edge).await?;
        let stale_nodes: Vec<_> = invalidated.iter().filter_map(|id| staged.get_node(id)).collect();
        write_freshness(&mut tx, stale_nodes).await?;
        write_task(&mut tx, &task).await?;
        commit_dag(tx, dag, staged).await?;
        self.clear_task_log_seq(task_id);
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

    pub async fn list_chat_messages(&self, limit: u32) -> RelayResult<Vec<AgentChatMessage>> {
        self.store.list_chat_messages(&self.repo_id, limit).await
    }

    pub async fn upsert_chat_message(&self, message: &AgentChatMessage) -> RelayResult<()> {
        self.check_repo(&message.repo_id)?;
        self.store.upsert_chat_message(message).await
    }

    pub async fn clear_chat_messages(&self) -> RelayResult<()> {
        self.store.clear_chat_messages(&self.repo_id).await
    }

    pub async fn mark_chat_approval_resolved(
        &self,
        task_id: &str,
        patch_id: Option<&str>,
        decision: &str,
    ) -> RelayResult<u32> {
        self.store
            .resolve_chat_approvals(&self.repo_id, task_id, patch_id, decision)
            .await
    }

    pub async fn upsert_trusted_peer(
        &self,
        peer: &TrustedPeerRecord,
    ) -> RelayResult<TrustedPeerRecord> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        let peer = peer.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let stored = engine.store.upsert_trusted_peer(&peer).await?;
            if stored.is_revoked() {
                engine
                    .outbox_counts
                    .write()
                    .expect("outbox counts lock poisoned")
                    .revoked_peers
                    .insert(stored.node_id.clone());
            }
            Ok(stored)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    pub async fn list_trusted_peers(&self) -> RelayResult<Vec<TrustedPeerRecord>> {
        self.store.list_trusted_peers().await
    }

    pub async fn get_trusted_peer(&self, node_id: &str) -> RelayResult<Option<TrustedPeerRecord>> {
        self.store.get_trusted_peer(node_id).await
    }

    pub async fn is_peer_revoked(&self, node_id: &str) -> RelayResult<bool> {
        self.store.is_peer_revoked(node_id).await
    }

    pub async fn touch_trusted_peer_seen(&self, node_id: &str) -> RelayResult<bool> {
        self.store.touch_trusted_peer_seen(node_id).await
    }

    pub async fn rename_trusted_peer(
        &self,
        node_id: &str,
        label: &str,
    ) -> RelayResult<TrustedPeerRecord> {
        self.store.rename_trusted_peer(node_id, label).await
    }

    pub async fn revoke_trusted_peer(&self, node_id: &str) -> RelayResult<TrustedPeerRecord> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        let node_id = node_id.to_owned();
        tokio::spawn(async move {
            let _guard = guard;
            let updated = engine.store.revoke_trusted_peer(&node_id).await?;
            engine
                .outbox_counts
                .write()
                .expect("outbox counts lock poisoned")
                .revoked_peers
                .insert(updated.node_id.clone());
            Ok(updated)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    pub async fn remove_trusted_peer(&self, node_id: &str) -> RelayResult<bool> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        let node_id = node_id.to_owned();
        tokio::spawn(async move {
            let _guard = guard;
            let removed = engine.store.remove_trusted_peer(&node_id).await?;
            if removed {
                engine
                    .outbox_counts
                    .write()
                    .expect("outbox counts lock poisoned")
                    .revoked_peers
                    .remove(&node_id);
            }
            Ok(removed)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    pub async fn enqueue_outbox(&self, payload: &Value) -> RelayResult<i64> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        let payload = payload.clone();
        // Once a write starts, publish its result even if its caller is cancelled.
        tokio::spawn(async move {
            let _guard = guard;
            let seq = engine.store.enqueue_outbox(&engine.repo_id, &payload).await?;
            engine.record_outbox_enqueued(None);
            Ok(seq)
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    pub async fn pending_outbox_count(&self) -> RelayResult<u32> {
        self.pending_outbox_count_for_peer(None).await
    }

    pub async fn pending_outbox_count_for_peer(&self, node_id: Option<&str>) -> RelayResult<u32> {
        self.store
            .count_repo_pending_outbox_for_peer(Some(&self.repo_id), node_id)
            .await
    }

    pub async fn mark_outbox_delivered(&self, seq: i64) -> RelayResult<()> {
        self.mark_outbox_delivered_inner(seq, None).await
    }

    pub(crate) async fn mark_peer_outbox_delivered(
        &self,
        seq: i64,
        node_id: &str,
    ) -> RelayResult<()> {
        self.mark_outbox_delivered_inner(seq, Some(node_id.to_owned()))
            .await
    }

    async fn mark_outbox_delivered_inner(
        &self,
        seq: i64,
        delivered_to: Option<String>,
    ) -> RelayResult<()> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let engine = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            if let Some(prior_recipient) = engine
                .store
                .mark_repo_outbox_delivered(Some(&engine.repo_id), seq, delivered_to.as_deref())
                .await?
            {
                engine
                    .outbox_counts
                    .write()
                    .expect("outbox counts lock poisoned")
                    .record_delivered(prior_recipient.as_deref());
            }
            Ok(())
        })
        .await
        .map_err(|err| RelayError::Internal(err.to_string()))?
    }

    #[cfg(test)]
    pub(crate) async fn list_pending_outbox(&self) -> RelayResult<Vec<(i64, String, Value)>> {
        let _guard = self.metadata_writes.lock().await;
        let items = self.store.list_pending_outbox(Some(&self.repo_id)).await?;
        let snapshot = self.store.load_outbox_snapshot(&self.repo_id).await?;
        *self
            .outbox_counts
            .write()
            .expect("outbox counts lock poisoned") = snapshot;
        Ok(items)
    }

    pub(crate) async fn list_peer_pending_outbox(
        &self,
        node_id: &str,
    ) -> RelayResult<Vec<(i64, Value)>> {
        self.store.list_peer_pending_outbox(&self.repo_id, node_id).await
    }

    pub(crate) async fn lock_outbox_flush(&self) -> MutexGuard<'_, ()> {
        self.outbox_flush.lock().await
    }

    #[cfg(test)]
    pub(crate) fn observed_outbox_count(&self) -> u32 {
        self.observed_outbox_count_for_peer(None)
    }

    pub(crate) fn observed_outbox_count_for_peer(&self, node_id: Option<&str>) -> u32 {
        self.outbox_counts
            .read()
            .expect("outbox counts lock poisoned")
            .count_for_peer(node_id)
    }

    fn record_outbox_enqueued(&self, node_id: Option<&str>) {
        self.outbox_counts
            .write()
            .expect("outbox counts lock poisoned")
            .record_enqueued(node_id);
    }

    pub async fn runtime_mode(&self) -> RuntimeMode {
        let _guard = self.metadata_writes.lock().await;
        self.observed_runtime_mode()
    }

    pub(crate) fn observed_runtime_mode(&self) -> RuntimeMode {
        *self.mode.borrow()
    }

    pub async fn set_runtime_mode(&self, mode: RuntimeMode) -> RelayResult<()> {
        let guard = self.metadata_writes.clone().lock_owned().await;
        let current = self.mode.clone();
        let repo_id = self.repo_id.clone();
        let store = self.store.clone();
        tokio::spawn(async move {
            let _guard = guard;
            store.put_runtime_mode(&repo_id, mode).await?;
            current.send_replace(mode);
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

    pub(crate) fn check_repo(&self, repo_id: &str) -> RelayResult<()> {
        if self.repo_id != repo_id {
            return Err(RelayError::Internal(format!(
                "repository {repo_id} does not match engine repository {}",
                self.repo_id,
            )));
        }
        Ok(())
    }
}

fn validate_approval_patch(
    record: &TaskRecord,
    patch_id: Option<&str>,
    exact_patch: bool,
) -> RelayResult<()> {
    let active_id = record
        .active_proposal
        .as_ref()
        .map(|patch| patch.patch_id.as_str());
    if (exact_patch || patch_id.is_some()) && active_id != patch_id {
        return Err(RelayError::ProposalNotFound(
            patch_id.unwrap_or("no proposal").into(),
        ));
    }
    Ok(())
}

fn approval_state(decision: ApprovalDecision) -> TaskState {
    match decision {
        ApprovalDecision::Approved => TaskState::Executing,
        ApprovalDecision::Rejected => TaskState::Rejected,
    }
}

fn queued_task(mut spec: TaskSpec) -> TaskRecord {
    if spec.task_id.is_nil() {
        spec.task_id = Uuid::now_v7();
    }
    let now = Utc::now();
    TaskRecord {
        spec,
        state: TaskState::Queued,
        active_proposal: None,
        preflight_summary: None,
        evidence_node_id: None,
        error_message: None,
        created_at: now,
        updated_at: now,
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
    mut tx: Transaction<'static, Sqlite>,
    mut dag: OwnedMutexGuard<CachedDag>,
    staged: DagEngine,
) -> RelayResult<()> {
    let revision = dag_revision(&mut tx).await?;
    tokio::spawn(async move {
        tx.commit().await?;
        *dag = CachedDag { graph: staged, revision };
        Ok(())
    })
    .await
    .map_err(|err| RelayError::Internal(err.to_string()))?
}

#[cfg(test)]
mod tests;
