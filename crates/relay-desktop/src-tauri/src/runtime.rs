mod network;

use crate::{DagStaleAlertEvent, emit_state};
use chrono::Utc;
use iroh::EndpointAddr;
use network::{Network, NodeEvents, fetch_dag, rpc_error};
use relay_core::agent::{LlmClient, LlmEndpointConfig, SelfHealingAgent, build_safe_dag_context};
use relay_core::contracts::{
    ApprovalDecision, DagDelta, Freshness, GraphNode, NodeKind, PatchKind, PeerStatus,
    ProposeWorktreePatchArgs, RunPreflightArgs, RuntimeMode, StalePropagationReport, TaskId,
    TaskRecord, TaskSpec, TaskState, WireRequest, WireResponse,
};
use relay_core::engine::RelayEngine;
use relay_core::error::{RelayError, RelayResult};
use relay_core::mcp::RelayMcpServer;
use relay_core::sandbox::{WorktreeSandbox, compute_env_fingerprint, load_manifest};
use relay_core::sync::RelayNodeEvent;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tauri_specta::Event;
use tokio::sync::Mutex;
use tokio::time::{self, MissedTickBehavior};
use uuid::Uuid;

const REPO_ID: &str = "forecast-bench";
const FIXTURE_PATH: &str = "bench/fixture-repo";
const PAIRING_WINDOW: Duration = Duration::from_secs(120);
const PEER_SYNC_INTERVAL: Duration = Duration::from_secs(3);

pub(crate) struct DesktopRuntime {
    pub(crate) engine: Arc<RelayEngine>,
    app: AppHandle,
    data_dir: PathBuf,
    sandbox: Option<WorktreeSandbox>,
    mcp: Arc<RelayMcpServer>,
    network: Mutex<Network>,
}

impl DesktopRuntime {
    pub(crate) async fn open(app: AppHandle) -> RelayResult<Arc<Self>> {
        let cwd = env::current_dir().ok();
        let starts = cwd
            .as_deref()
            .into_iter()
            .chain([Path::new(env!("CARGO_MANIFEST_DIR"))]);
        let ancestors: Vec<_> = starts.flat_map(Path::ancestors).collect();
        let workspace = ancestors
            .iter()
            .copied()
            .find(|dir| dir.join(FIXTURE_PATH).join("relay.toml").is_file());
        let repo_root = match env_path("RELAY_REPO_ROOT") {
            Some(root) if root.is_dir() => Some(std::path::absolute(root)?),
            Some(root) => {
                return Err(RelayError::Internal(format!(
                    "RELAY_REPO_ROOT is not a directory: {}",
                    root.display()
                )));
            }
            None => workspace
                .map(|dir| dir.join(FIXTURE_PATH))
                .or_else(|| {
                    ancestors.iter().find_map(|dir| {
                        dir.join("relay.toml").is_file().then(|| dir.to_path_buf())
                    })
                }),
        };
        let data_dir = resolve_data_dir(workspace.or(repo_root.as_deref()).or(cwd.as_deref())).await?;
        let engine = Arc::new(
            RelayEngine::open_with_default_mode(
                REPO_ID,
                data_dir.join("relay.db"),
                repo_root.clone(),
                RuntimeMode::LocalDualSandbox,
            )
            .await?,
        );

        // A fixture inside a parent Git checkout can supply metadata, but its
        // worktree would check out the parent and lack relay.toml at the root.
        let repo = repo_root.map(|root| WorktreeSandbox::new(root, data_dir.join("worktrees")));
        let head = if let Some(repo) = &repo {
            match repo.head_commit_sha().await {
                Ok(head) => Some(head),
                Err(error) => {
                    tracing::debug!(%error, "Repository HEAD is unavailable");
                    None
                }
            }
        } else {
            None
        };
        let sandbox = repo
            .as_ref()
            .filter(|repo| {
                head.is_some()
                    && repo.repo_root().join(".git").exists()
                    && load_manifest(repo.repo_root()).is_ok()
            })
            .cloned();
        let mode = engine.runtime_mode().await;
        let (network, events) = Network::start(mode, &data_dir, &engine, sandbox.clone()).await?;

        if let (Some(repo), Some(head)) = (&repo, head)
            && engine.dag_snapshot(true).await.nodes.is_empty()
        {
            let origin = network.executor_node().id().to_string();
            let seed = async {
                let node = artifact_node(repo, head, origin).await?;
                engine.upsert_node_and_edges(node, Vec::new()).await?;
                Ok::<_, RelayError>(())
            };
            if let Err(error) = seed.await {
                tracing::warn!(%error, "Unable to seed the repository HEAD artifact");
            }
            network.sync_loopback_head().await;
        }

        let mcp = Arc::new(RelayMcpServer::new(
            engine.clone(),
            sandbox.clone(),
            Some(network.executor_node().clone()),
        ));
        let runtime = Arc::new(Self {
            engine,
            app,
            data_dir,
            sandbox,
            mcp,
            network: Mutex::new(network),
        });
        runtime.listen(events, mode);
        runtime.start_peer_sync();
        Ok(runtime)
    }

    pub(crate) async fn status(&self) -> PeerStatus {
        let network = self.network.lock().await;
        let status = network.local.peer_status();
        if status.connected || network.loopback_host.is_none() {
            return status;
        }

        // Restore an idle loopback connection before reporting its current status.
        network.sync_loopback_head().await;
        network.local.peer_status()
    }

    pub(crate) async fn switch_mode(self: &Arc<Self>, mode: RuntimeMode) -> RelayResult<RuntimeMode> {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut network = runtime.network.lock().await;
            if network.mode == mode {
                return Ok(mode);
            }
            let (next, events) =
                Network::start(mode, &runtime.data_dir, &runtime.engine, runtime.sandbox.clone())
                    .await?;
            if let Err(error) = runtime.engine.set_runtime_mode(mode).await {
                next.shutdown().await;
                return Err(error);
            }
            runtime.mcp.set_node(next.executor_node().clone());
            let previous = std::mem::replace(&mut *network, next);
            runtime.listen(events, mode);
            previous.shutdown().await;
            emit_state(&runtime.app, "runtime_mode_changed", format!("Runtime mode is {mode:?}"));
            Ok(mode)
        })
        .await
        .map_err(|error| RelayError::Internal(error.to_string()))?
    }

    pub(crate) async fn pairing_ticket(&self) -> RelayResult<String> {
        let network = self.network.lock().await;
        let ticket = serde_json::to_string(&network.local.addr())?;
        if network.mode != RuntimeMode::LocalDualSandbox {
            network.local.open_pairing_window(PAIRING_WINDOW);
        }
        Ok(ticket)
    }

    pub(crate) async fn connect_ticket(&self, ticket_json: &str) -> RelayResult<PeerStatus> {
        let address: EndpointAddr = serde_json::from_str(ticket_json)?;
        let mut network = self.network.lock().await;
        if network.mode == RuntimeMode::LocalDualSandbox {
            return Err(RelayError::Transport(
                "Switch to Client or Host mode to pair with another machine".into(),
            ));
        }
        if address.id == network.local.id() {
            return Err(RelayError::Transport("This ticket belongs to the local node".into()));
        }

        let local_id = network.local.id().to_string();
        if let Err(error) = network.local.ping_peer(address.clone(), &local_id).await {
            if let Some(previous) = &network.remote {
                network.local.set_remote_peer(previous.clone());
            }
            return Err(error);
        }
        if network.remote.as_ref().map(|peer| peer.id) != Some(address.id) {
            network.last_remote_dag = None;
        }
        network.remote = Some(address.clone());
        emit_state(&self.app, "peer_paired", format!("Paired with {}", address.id));

        let delivered = network.local.flush_outbox(address).await?;
        if delivered > 0 {
            emit_state(&self.app, "outbox_flushed", format!("Delivered {delivered} queued requests"));
        }
        if network.mode == RuntimeMode::Client {
            self.refresh_from_host(&mut network).await?;
        }
        Ok(network.local.peer_status())
    }

    pub(crate) async fn submit(self: &Arc<Self>, mut spec: TaskSpec) -> RelayResult<TaskRecord> {
        if spec.task_id.is_nil() {
            spec.task_id = Uuid::now_v7();
        }
        let network = self.network.lock().await;
        let id = spec.task_id;
        let result = match network.mode {
            RuntimeMode::LocalDualSandbox => {
                let host = network.loopback_host.as_ref().expect("loopback host");
                // The Host's incoming RPC notification starts the preflight.
                return network.local.submit_remote_task(host.addr(), spec).await;
            }
            RuntimeMode::Host => {
                let (record, inserted) = self.engine.submit_task(spec).await?;
                if inserted {
                    self.task_submitted(id, true);
                }
                return Ok(record);
            }
            RuntimeMode::Client => {
                if let Some(address) = &network.remote {
                    let request = WireRequest::SubmitTask { spec: spec.clone() };
                    match network.local.send_rpc(address.clone(), request).await {
                        Ok(response) => {
                            let record = task_response(response)?;
                            self.engine.mirror_peer_task(&record).await?;
                            record
                        }
                        Err(RelayError::Transport(error)) => {
                            tracing::debug!(%error, "Queueing task for the disconnected host");
                            self.engine.queue_peer_task(spec).await?
                        }
                        Err(error) => return Err(error),
                    }
                } else {
                    self.engine.queue_peer_task(spec).await?
                }
            }
        };
        self.task_submitted(id, false);
        Ok(result)
    }

    pub(crate) async fn resolve_approval(
        self: &Arc<Self>,
        task_id: TaskId,
        decision: ApprovalDecision,
    ) -> RelayResult<TaskRecord> {
        let network = self.network.lock().await;
        let task = self
            .engine
            .get_task(task_id)
            .await?
            .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))?;
        let patch_id = task.active_proposal.as_ref().map(|patch| patch.patch_id.as_str());
        let remote = if let Some(host) = &network.loopback_host {
            Some(host.addr())
        } else if network.mode == RuntimeMode::Client {
            network.remote.clone()
        } else {
            None
        };

        if let Some(address) = remote {
            if network.mode == RuntimeMode::LocalDualSandbox {
                return network
                    .local
                    .resolve_remote_approval(address, task_id, patch_id.map(str::to_owned), decision)
                    .await;
            }
            let request = WireRequest::ResolveApproval {
                task_id,
                patch_id: patch_id.map(str::to_owned),
                decision,
                comment: None,
            };
            match network.local.send_rpc(address, request).await {
                Ok(response) => {
                    let record = task_response(response)?;
                    self.engine.mirror_peer_task(&record).await?;
                    self.approval_resolved(task_id, decision, false);
                    return Ok(record);
                }
                Err(RelayError::Transport(error)) => {
                    tracing::debug!(%error, "Queueing approval for the disconnected host");
                }
                Err(error) => return Err(error),
            }
        }
        let record = if network.mode == RuntimeMode::Client {
            self.engine.queue_peer_approval(task_id, patch_id, decision).await?
        } else {
            self.engine.resolve_ui_approval(task_id, patch_id, decision).await?
        };
        self.approval_resolved(task_id, decision, network.mode == RuntimeMode::Host);
        Ok(record)
    }

    pub(crate) async fn supersede_artifact(
        &self,
        old_artifact_id: &str,
        new_commit_sha: String,
        reason: String,
    ) -> RelayResult<StalePropagationReport> {
        let origin = self.network.lock().await.local.id().to_string();
        let node = if let Some(root) = self.engine.repo_root() {
            let repo = WorktreeSandbox::new(root, self.data_dir.join("worktrees"));
            artifact_node(&repo, new_commit_sha, origin).await?
        } else {
            let snapshot = self.engine.dag_snapshot(true).await;
            let fingerprint = snapshot.nodes.into_iter().find_map(|node| match node.payload {
                NodeKind::Artifact { env_fingerprint, .. } if node.id == old_artifact_id => {
                    Some(env_fingerprint)
                }
                _ => None,
            });
            let fingerprint =
                fingerprint.ok_or_else(|| RelayError::NodeNotFound(old_artifact_id.into()))?;
            make_artifact(new_commit_sha, fingerprint, false, Vec::new(), origin)
        };
        let report = self.engine.supersede_artifact(old_artifact_id, node, reason).await?;
        if let Err(error) = (DagStaleAlertEvent { report: report.clone() }).emit(&self.app) {
            tracing::warn!(%error, "Unable to emit DAG staleness event");
        }
        emit_state(&self.app, "dag_changed", "Artifact superseded");
        Ok(report)
    }

    fn listen(self: &Arc<Self>, listeners: Vec<NodeEvents>, mode: RuntimeMode) {
        for mut events in listeners {
            let runtime = Arc::downgrade(self);
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    let Some(runtime) = runtime.upgrade() else {
                        break;
                    };
                    let execute = mode != RuntimeMode::Client;
                    match event {
                        RelayNodeEvent::TaskSubmitted(id) => runtime.task_submitted(id, execute),
                        RelayNodeEvent::ApprovalResolved(id, decision) => {
                            runtime.approval_resolved(id, decision, execute);
                        }
                        RelayNodeEvent::DagChanged => {
                            emit_state(&runtime.app, "dag_changed", "DAG updated by peer");
                        }
                        RelayNodeEvent::PeerPaired => {
                            emit_state(&runtime.app, "peer_paired", "Peer connected");
                        }
                    }
                }
            });
        }
    }

    fn task_submitted(self: &Arc<Self>, id: TaskId, execute: bool) {
        emit_state(&self.app, "task_submitted", format!("Task {id} submitted"));
        if execute && self.sandbox.is_some() {
            let runtime = self.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = runtime.preflight(id).await {
                    tracing::warn!(task_id = %id, %error, "Task preflight or diagnosis failed");
                    emit_state(&runtime.app, "task_preflight_failed", format!("Task {id}: {error}"));
                }
            });
        }
    }

    async fn preflight(&self, id: TaskId) -> RelayResult<()> {
        self.engine
            .transition_task(id, TaskState::PreflightRunning, None, None)
            .await?;
        emit_state(
            &self.app,
            "task_preflight_started",
            format!("Preflight started for task {id}"),
        );
        let task_id = id.to_string();
        let report = self
            .mcp
            .handle_run_preflight(RunPreflightArgs { task_id: task_id.clone() })
            .await?;
        emit_state(&self.app, "task_preflight", format!("Preflight finished for task {id}"));
        if !report.smoke_passed {
            let task = self
                .engine
                .get_task(id)
                .await?
                .ok_or_else(|| RelayError::TaskNotFound(task_id.clone()))?;
            if task.state != TaskState::PreflightRunning {
                return Ok(());
            }
            let context = format!(
                "Repository {}\nBase commit {}\nRequested preset {}\n{}",
                task.spec.repo_id,
                task.spec.base_commit_sha,
                task.spec.command_preset,
                build_safe_dag_context(&self.engine.dag_snapshot(true).await),
            );
            let llm = LlmClient::new(LlmEndpointConfig::default(), Duration::from_secs(30))?;
            let proposal = SelfHealingAgent::new(llm)
                .diagnose_preflight_failure(&report, &context)
                .await?;
            let patch_kind = match proposal.patch_kind {
                PatchKind::UvSpec => "uv_spec",
                PatchKind::CodeDiff => "code_diff",
            };
            self.mcp
                .handle_propose_patch(ProposeWorktreePatchArgs {
                    task_id,
                    patch_kind: patch_kind.into(),
                    summary: proposal.summary,
                    content: proposal.content,
                })
                .await?;
            emit_state(&self.app, "patch_proposed", format!("Verified patch for task {id}"));
        }
        Ok(())
    }

    fn approval_resolved(self: &Arc<Self>, id: TaskId, decision: ApprovalDecision, execute: bool) {
        emit_state(&self.app, "approval_resolved", format!("Task {id}: {decision:?}"));
        if execute && self.sandbox.is_some() {
            let runtime = self.clone();
            tauri::async_runtime::spawn(async move {
                match runtime.mcp.finish_ui_approval(id, decision).await {
                    Ok(record) => emit_state(
                        &runtime.app,
                        "task_updated",
                        format!("Task {id}: {:?}", record.state),
                    ),
                    Err(error) => {
                        tracing::warn!(task_id = %id, %error, "Task approval could not finish");
                        emit_state(&runtime.app, "task_execution_failed", format!("Task {id}: {error}"));
                    }
                }
            });
        }
    }

    fn start_peer_sync(self: &Arc<Self>) {
        let runtime = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            let mut interval = time::interval(PEER_SYNC_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(runtime) = runtime.upgrade() else {
                    break;
                };
                let Ok(mut network) = runtime.network.try_lock() else {
                    continue;
                };
                if network.mode == RuntimeMode::Client && network.remote.is_some() {
                    let status_before = network.local.peer_status();
                    if let Err(error) = runtime.refresh_from_host(&mut network).await {
                        tracing::debug!(%error, "Unable to refresh state from the paired host");
                    }
                    let status_after = network.local.peer_status();
                    if status_before.connected != status_after.connected
                        || status_before.transport_path != status_after.transport_path
                        || status_before.pending_outbox_count != status_after.pending_outbox_count
                    {
                        emit_state(&runtime.app, "peer_status_changed", "Peer status changed");
                    }
                }
            }
        });
    }

    async fn refresh_from_host(&self, network: &mut Network) -> RelayResult<()> {
        let Some(address) = &network.remote else {
            return Ok(());
        };
        if network.local.peer_status().pending_outbox_count != 0 {
            network.local.flush_outbox(address.clone()).await?;
        }
        let snapshot = fetch_dag(&network.local, address.clone()).await?;
        if network.last_remote_dag.as_ref().is_none_or(|previous| {
            previous.nodes != snapshot.nodes || previous.edges != snapshot.edges
        }) {
            let current = self.engine.dag_snapshot(true).await;
            if current.nodes != snapshot.nodes || current.edges != snapshot.edges {
                self.engine
                    .apply_dag_delta(DagDelta {
                        repo_id: snapshot.repo_id.clone(),
                        upsert_nodes: snapshot.nodes.clone(),
                        upsert_edges: snapshot.edges.clone(),
                    })
                    .await?;
                emit_state(&self.app, "dag_changed", "DAG refreshed from host");
            }
            network.last_remote_dag = Some(snapshot);
        }
        let mut changed = false;
        for task in self.engine.list_tasks().await? {
            let request = WireRequest::FetchTaskState { task_id: task.spec.task_id };
            match network.local.send_rpc(address.clone(), request).await? {
                WireResponse::TaskState(record) => {
                    changed |= self.engine.mirror_peer_task(&record).await?;
                }
                WireResponse::Error { code, .. } if code == "not_found" => {}
                response => return Err(rpc_error(response, "task state")),
            }
        }
        if changed {
            emit_state(&self.app, "task_updated", "Task state refreshed from host");
        }
        Ok(())
    }
}

fn task_response(response: WireResponse) -> RelayResult<TaskRecord> {
    match response {
        WireResponse::TaskState(record) => Ok(record),
        response => Err(rpc_error(response, "task state")),
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from)
}

async fn resolve_data_dir(parent: Option<&Path>) -> RelayResult<PathBuf> {
    let dir = if let Some(dir) = env_path("RELAY_DATA_DIR") {
        dir
    } else if let Some(parent) = parent {
        let dir = parent.join(".relay").join("desktop");
        match tokio::fs::create_dir_all(&dir).await {
            Ok(()) => return Ok(std::path::absolute(dir)?),
            Err(error) => tracing::debug!(%error, path = %dir.display(), "Workspace data directory unavailable"),
        }
        env::temp_dir().join("relay-desktop")
    } else {
        env::temp_dir().join("relay-desktop")
    };
    let dir = std::path::absolute(dir)?;
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir)
}

async fn artifact_node(
    repo: &WorktreeSandbox,
    commit_sha: String,
    device_origin: String,
) -> RelayResult<GraphNode> {
    let fingerprint = compute_env_fingerprint(repo.repo_root())?;
    let (changed_files, dirty) =
        tokio::join!(repo.changed_files_in_commit(&commit_sha), repo.is_working_tree_dirty());
    Ok(make_artifact(
        commit_sha,
        fingerprint,
        dirty.unwrap_or_default(),
        changed_files.unwrap_or_default(),
        device_origin,
    ))
}

fn make_artifact(
    commit_sha: String,
    env_fingerprint: String,
    working_tree_dirty: bool,
    changed_files: Vec<String>,
    device_origin: String,
) -> GraphNode {
    let now = Utc::now();
    let short_sha: String = commit_sha.chars().take(7).collect();
    GraphNode {
        id: format!("artifact:{commit_sha}"),
        label: format!("Commit {short_sha}"),
        payload: NodeKind::Artifact {
            git_commit_sha: commit_sha,
            env_fingerprint,
            working_tree_dirty,
            changed_files,
        },
        freshness: Freshness::Fresh,
        device_origin,
        created_at: now,
        updated_at: now,
    }
}
