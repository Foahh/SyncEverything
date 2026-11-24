mod copilot;
mod network;

use crate::{
    DagStaleAlertEvent, LlmSettingsUpdate, PresetInfo, WorkspaceInfo, emit_state,
};
use chrono::Utc;
use network::{Network, NodeEvents, fetch_dag, rpc_error};
use relay_core::agent::{
    HostTaskOutcome, LlmClient, LlmEndpointConfig, LlmProbeResult, LlmSettingsView,
    load_llm_config, run_host_task_pipeline, save_llm_config,
};
use relay_core::contracts::{
    ApprovalDecision, DagDelta, Freshness, GraphNode, NodeKind, PeerStatus, RuntimeMode,
    StalePropagationReport, TaskId, TaskRecord, TaskSpec, WireRequest, WireResponse,
};
use relay_core::engine::RelayEngine;
use relay_core::error::{RelayError, RelayResult};
use relay_core::mcp::RelayMcpServer;
use relay_core::sandbox::{
    HeadSyncOutcome, WorktreeSandbox, build_git_artifact_node, load_manifest,
    sync_git_head_into_engine,
};
use relay_core::sync::{RelayNodeEvent, parse_pairing_ticket};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;
use tokio::sync::{Mutex, RwLock, watch};
use tokio::time::{self, MissedTickBehavior};
use uuid::Uuid;

const DEFAULT_REPO_ID: &str = "default-workspace";
const PAIRING_WINDOW: Duration = Duration::from_secs(120);
const PEER_SYNC_INTERVAL: Duration = Duration::from_secs(3);
const GIT_WATCH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorkspacePrefs {
    repo_root: Option<String>,
}

pub(crate) struct DesktopRuntime {
    pub(crate) engine: Arc<RelayEngine>,
    app: AppHandle,
    base_data_dir: PathBuf,
    repo_data_dir: PathBuf,
    sandbox: Option<WorktreeSandbox>,
    workspace: WorkspaceInfo,
    mcp: Arc<RelayMcpServer>,
    network: Mutex<Option<Network>>,
    llm_config: RwLock<LlmEndpointConfig>,
    active_chat_turn: Mutex<Option<copilot::ChatTurn>>,
    shutdown: watch::Sender<bool>,
}

impl DesktopRuntime {
    pub(crate) async fn open(
        app: AppHandle,
        explicit_repo_root: Option<PathBuf>,
    ) -> RelayResult<Arc<Self>> {
        let base_data_dir = resolve_base_data_dir(&app).await?;

        let repo_root = if let Some(explicit) = explicit_repo_root {
            let abs = std::path::absolute(explicit)?;
            if !abs.is_dir() {
                return Err(RelayError::Internal(format!(
                    "Repository path is not a directory: {}",
                    abs.display()
                )));
            }
            Some(abs)
        } else if let Some(env_root) = env_path("RELAY_REPO_ROOT") {
            if !env_root.is_dir() {
                return Err(RelayError::Internal(format!(
                    "RELAY_REPO_ROOT is not a directory: {}",
                    env_root.display()
                )));
            }
            Some(std::path::absolute(env_root)?)
        } else {
            load_workspace_prefs(&base_data_dir)
                .repo_root
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
        };

        let workspace_info = inspect_workspace(repo_root.as_deref());
        let repo_id = workspace_info.repo_id.clone();
        let repo_data_dir = base_data_dir
            .join("repos")
            .join(sanitize_repo_dir(&repo_id));
        tokio::fs::create_dir_all(&repo_data_dir).await?;

        let engine = Arc::new(
            RelayEngine::open_with_default_mode(
                &repo_id,
                repo_data_dir.join("relay.db"),
                repo_root.clone(),
                RuntimeMode::LocalDualSandbox,
            )
            .await?,
        );

        let sandbox = repo_root.as_ref()
            .filter(|root| root.join(".git").exists())
            .map(|root| WorktreeSandbox::new(root, repo_data_dir.join("worktrees")));

        let mode = engine.runtime_mode().await;
        let (network, events) =
            Network::start(mode, &repo_data_dir, &engine, sandbox.clone()).await?;

        let origin = network.executor_node().id().to_string();
        if let Some(sb) = &sandbox {
            match sync_git_head_into_engine(&engine, sb, origin).await {
                Ok(HeadSyncOutcome::Seeded { .. } | HeadSyncOutcome::Superseded { .. }) => {
                    network.sync_loopback_head(engine.repo_id()).await;
                }
                Ok(HeadSyncOutcome::Unchanged) => {}
                Err(error) => {
                    tracing::warn!(%error, "Unable to sync initial repository HEAD artifact");
                }
            }
        }

        let llm_config = load_llm_config(&base_data_dir.join("llm-settings.json"))
            .unwrap_or_else(|_| LlmEndpointConfig::from_env_or_default());
        let mcp = Arc::new(RelayMcpServer::new(
            engine.clone(),
            sandbox.clone(),
            Some(network.executor_node().clone()),
        ));
        let (shutdown, _) = watch::channel(false);
        let runtime = Arc::new(Self {
            engine,
            app,
            base_data_dir,
            repo_data_dir,
            sandbox,
            workspace: workspace_info,
            mcp,
            network: Mutex::new(Some(network)),
            llm_config: RwLock::new(llm_config),
            active_chat_turn: Mutex::new(None),
            shutdown,
        });
        runtime.listen(events, mode);
        runtime.start_peer_sync();
        runtime.start_git_head_watcher();
        Ok(runtime)
    }

    pub(crate) async fn shutdown(&self) {
        let _ = self.shutdown.send(true);
        self.cancel_chat_turn().await;
        if let Some(network) = self.network.lock().await.take() {
            network.shutdown().await;
        }
    }

    pub(crate) fn workspace_info(&self) -> WorkspaceInfo {
        let mut info = self.workspace.clone();
        if let Some(root) = self.engine.repo_root() {
            let refreshed = inspect_workspace(Some(root));
            if refreshed.repo_id == info.repo_id {
                info = refreshed;
            }
        }
        if info.repo_root.is_some() {
            info.mcp_data_dir = Some(self.repo_data_dir.to_string_lossy().into_owned());
        }
        info
    }

    pub(crate) fn matches_workspace(&self, root: &Path) -> bool {
        self.engine.repo_root().is_some_and(|active| {
            same_directory(active, root) && inspect_workspace(Some(root)).repo_id == self.engine.repo_id()
        })
    }

    pub(crate) fn shares_storage(&self, root: &Path) -> bool {
        let id = inspect_workspace(Some(root)).repo_id;
        let target = self.base_data_dir.join("repos").join(sanitize_repo_dir(&id));
        same_directory(&target, &self.repo_data_dir)
    }

    pub(crate) fn persist_repo_preference(&self, repo_root: &Path) -> RelayResult<()> {
        let prefs = WorkspacePrefs {
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
        };
        let path = self.base_data_dir.join("workspace.json");
        std::fs::write(path, serde_json::to_string_pretty(&prefs)?)?;
        Ok(())
    }

    pub(crate) async fn llm_settings_view(&self) -> LlmSettingsView {
        self.llm_config.read().await.redacted_view()
    }

    pub(crate) async fn save_llm_settings(
        &self,
        update: LlmSettingsUpdate,
    ) -> RelayResult<LlmSettingsView> {
        let next = self.merge_llm_update(update).await;
        // Validate URL & env syntax before saving.
        let _ = LlmClient::new(next.clone(), Duration::from_secs(5))?;
        save_llm_config(&self.base_data_dir.join("llm-settings.json"), &next)?;
        let view = next.redacted_view();
        *self.llm_config.write().await = next;
        emit_state(&self.app, "llm_settings_changed", "LLM settings saved");
        Ok(view)
    }

    pub(crate) async fn test_llm_connection(
        &self,
        update: Option<LlmSettingsUpdate>,
    ) -> RelayResult<LlmProbeResult> {
        let cfg = match update {
            Some(update) => self.merge_llm_update(update).await,
            None => self.llm_config.read().await.clone(),
        };
        let client = LlmClient::new(cfg, Duration::from_secs(12))?;
        Ok(client.probe().await)
    }

    async fn merge_llm_update(&self, update: LlmSettingsUpdate) -> LlmEndpointConfig {
        let current = self.llm_config.read().await.clone();
        let api_key = match update.api_key {
            None => current.api_key,
            Some(ref value) if value.trim().is_empty() => None,
            Some(value) => Some(value.trim().to_owned()),
        };
        let api_key_env = update
            .api_key_env
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        LlmEndpointConfig {
            tier: update.tier,
            base_url: update.base_url.trim().to_owned(),
            model: update.model.trim().to_owned(),
            api_key_env,
            api_key,
        }
    }

    pub(crate) async fn status(&self) -> PeerStatus {
        let guard = self.network.lock().await;
        let Some(network) = guard.as_ref() else {
            return PeerStatus {
                mode: self.engine.runtime_mode().await,
                local_node_id: String::new(),
                remote_node_id: None,
                transport_path: relay_core::contracts::TransportPath::Offline,
                connected: false,
                rtt_ms: None,
                last_synced_commit: None,
                pending_outbox_count: self.engine.pending_outbox_count().await.unwrap_or(0),
            };
        };
        let status = network.local.peer_status();
        if status.connected || network.loopback_host.is_none() {
            return status;
        }
        network.sync_loopback_head(self.engine.repo_id()).await;
        network.local.peer_status()
    }

    pub(crate) async fn switch_mode(self: &Arc<Self>, mode: RuntimeMode) -> RelayResult<RuntimeMode> {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut guard = runtime.network.lock().await;
            let Some(current) = guard.as_ref() else {
                return Err(RelayError::Transport("Runtime is shutting down".into()));
            };
            if current.mode == mode {
                return Ok(mode);
            }
            let (next, events) = Network::start(
                mode,
                &runtime.repo_data_dir,
                &runtime.engine,
                runtime.sandbox.clone(),
            )
            .await?;
            if let Err(error) = runtime.engine.set_runtime_mode(mode).await {
                next.shutdown().await;
                return Err(error);
            }
            runtime.mcp.set_node(next.executor_node().clone());
            let previous = guard.replace(next);
            runtime.listen(events, mode);
            if let Some(previous) = previous {
                previous.shutdown().await;
            }
            emit_state(
                &runtime.app,
                "runtime_mode_changed",
                format!("Runtime mode is {mode:?}"),
            );
            Ok(mode)
        })
        .await
        .map_err(|error| RelayError::Internal(error.to_string()))?
    }

    pub(crate) async fn pairing_ticket(&self) -> RelayResult<String> {
        if self.engine.repo_root().is_none() {
            return Err(RelayError::Git("Open the same Git repository on both devices before pairing.".into()));
        }
        let guard = self.network.lock().await;
        let network = guard
            .as_ref()
            .ok_or_else(|| RelayError::Transport("Runtime is shutting down".into()))?;
        if network.mode == RuntimeMode::LocalDualSandbox {
            return Err(RelayError::Transport("Choose a device role before pairing".into()));
        }
        let ticket = serde_json::to_string(&network.local.addr())?;
        network.local.open_pairing_window(PAIRING_WINDOW);
        Ok(ticket)
    }

    pub(crate) async fn connect_ticket(&self, ticket_json: &str) -> RelayResult<PeerStatus> {
        if self.engine.repo_root().is_none() {
            return Err(RelayError::Git("Open the same Git repository on both devices before pairing.".into()));
        }
        let address = parse_pairing_ticket(ticket_json)?;
        let mut guard = self.network.lock().await;
        let network = guard
            .as_mut()
            .ok_or_else(|| RelayError::Transport("Runtime is shutting down".into()))?;
        if network.mode == RuntimeMode::LocalDualSandbox {
            return Err(RelayError::Transport(
                "Switch to Client or Host mode to pair with another machine".into(),
            ));
        }
        if address.id == network.local.id() {
            return Err(RelayError::Transport(
                "This ticket belongs to the local node".into(),
            ));
        }

        let local_id = network.local.id().to_string();
        if let Err(error) = network.local.ping_peer(address.clone(), &local_id).await {
            if let Some(previous) = &network.remote {
                network.local.set_remote_peer(previous.clone());
            }
            return Err(error);
        }
        if network.remote.as_ref().map(|peer| peer.id) != Some(address.id) {
            network.last_pushed_commit = None;
        }
        network.remote = Some(address.clone());
        emit_state(&self.app, "peer_paired", format!("Paired with {}", address.id));

        if network.mode == RuntimeMode::Client {
            self.refresh_from_host(network).await?;
        } else {
            network.local.flush_outbox(address).await?;
        }
        Ok(network.local.peer_status())
    }

    pub(crate) async fn submit(self: &Arc<Self>, mut spec: TaskSpec) -> RelayResult<TaskRecord> {
        if spec.task_id.is_nil() {
            spec.task_id = Uuid::now_v7();
        }
        if spec.repo_id.trim().is_empty() {
            spec.repo_id = self.engine.repo_id().to_owned();
        }
        let mut guard = self.network.lock().await;
        let network = guard
            .as_mut()
            .ok_or_else(|| RelayError::Transport("Runtime is shutting down".into()))?;
        let id = spec.task_id;
        if network.mode != RuntimeMode::Client {
            let sandbox = self.sandbox.as_ref().ok_or_else(|| RelayError::Git(
                "Open a Git repository before running a task.".into(),
            ))?;
            load_manifest(sandbox.repo_root())?;
            sandbox.head_commit_sha().await.map_err(|_| RelayError::Git(
                "Create the first Git commit before running a task.".into(),
            ))?;
        }
        let result = match network.mode {
            RuntimeMode::LocalDualSandbox => {
                let host = network.loopback_host.as_ref().expect("loopback host");
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
                if let Some(address) = network.remote.clone() {
                    let request = WireRequest::SubmitTask { spec: spec.clone() };
                    let send = async {
                        self.sync_client_before_dispatch(network, &address).await?;
                        network.local.send_rpc(address, request).await
                    };
                    match send.await {
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
        patch_id: Option<String>,
        decision: ApprovalDecision,
    ) -> RelayResult<TaskRecord> {
        let guard = self.network.lock().await;
        let network = guard
            .as_ref()
            .ok_or_else(|| RelayError::Transport("Runtime is shutting down".into()))?;
        let remote = if let Some(host) = &network.loopback_host {
            Some(host.addr())
        } else if network.mode == RuntimeMode::Client {
            network.remote.clone()
        } else {
            None
        };

        if let Some(address) = remote {
            if network.mode == RuntimeMode::LocalDualSandbox {
                let record = network
                    .local
                    .resolve_remote_approval(
                        address,
                        task_id,
                        patch_id,
                        decision,
                    )
                    .await?;
                return Ok(record);
            }
            let request = WireRequest::ResolveApproval {
                task_id,
                patch_id: patch_id.clone(),
                decision,
                comment: None,
            };
            match network.local.send_rpc(address, request).await {
                Ok(response) => {
                    let record = task_response(response)?;
                    self.engine.mirror_peer_task(&record).await?;
                    self.approval_resolved(task_id, patch_id, decision, false);
                    return Ok(record);
                }
                Err(RelayError::Transport(error)) => {
                    tracing::debug!(%error, "Queueing approval for the disconnected host");
                }
                Err(error) => return Err(error),
            }
        }
        let record = if network.mode == RuntimeMode::Client {
            self.engine
                .queue_peer_approval(task_id, patch_id.as_deref(), decision)
                .await?
        } else {
            self.engine
                .resolve_ui_approval(task_id, patch_id.as_deref(), decision)
                .await?
        };
        self.approval_resolved(task_id, patch_id, decision, network.mode == RuntimeMode::Host);
        Ok(record)
    }

    async fn sync_chat_approval_card(
        &self,
        task_id: TaskId,
        patch_id: Option<&str>,
        decision: ApprovalDecision,
    ) {
        let label = match decision {
            ApprovalDecision::Approved => "approved",
            ApprovalDecision::Rejected => "rejected",
        };
        if let Ok(updated) = self
            .engine
            .mark_chat_approval_resolved(&task_id.to_string(), patch_id, label)
            .await
            && updated > 0
        {
            emit_state(&self.app, "chat_updated", "Updated Copilot approval card");
        }
    }

    pub(crate) async fn supersede_artifact(
        &self,
        old_artifact_id: &str,
        new_commit_sha: String,
        reason: String,
    ) -> RelayResult<StalePropagationReport> {
        let origin = self
            .network
            .lock()
            .await
            .as_ref()
            .map_or_else(|| "local".into(), |net| net.local.id().to_string());
        let node = if let Some(root) = self.engine.repo_root() {
            let repo = WorktreeSandbox::new(root, self.repo_data_dir.join("worktrees"));
            match build_git_artifact_node(&repo, new_commit_sha.clone(), origin.clone()).await {
                Ok(node) => node,
                Err(_) => fallback_artifact(&self.engine, old_artifact_id, new_commit_sha, origin).await?,
            }
        } else {
            fallback_artifact(&self.engine, old_artifact_id, new_commit_sha, origin).await?
        };
        let report = self
            .engine
            .supersede_artifact(old_artifact_id, node, reason)
            .await?;
        if let Err(error) = (DagStaleAlertEvent {
            report: report.clone(),
        })
        .emit(&self.app)
        {
            tracing::warn!(%error, "Unable to emit DAG staleness event");
        }
        emit_state(&self.app, "dag_changed", "Artifact superseded");
        Ok(report)
    }

    pub(crate) async fn check_git_head(&self) -> RelayResult<bool> {
        let Some(sandbox) = &self.sandbox else {
            return Ok(false);
        };
        let origin = self
            .network
            .lock()
            .await
            .as_ref()
            .map_or_else(|| "local".into(), |net| net.local.id().to_string());

        self.observe_git_head(sandbox, origin).await
    }

    async fn observe_git_head(&self, sandbox: &WorktreeSandbox, origin: String) -> RelayResult<bool> {
        match sync_git_head_into_engine(&self.engine, sandbox, origin).await? {
            HeadSyncOutcome::Unchanged => Ok(false),
            HeadSyncOutcome::Seeded { commit_sha } => {
                let short_sha: String = commit_sha.chars().take(7).collect();
                emit_state(
                    &self.app,
                    "dag_changed",
                    format!("Synced Git HEAD {short_sha}"),
                );
                Ok(true)
            }
            HeadSyncOutcome::Superseded { commit_sha, report } => {
                let short_sha: String = commit_sha.chars().take(7).collect();
                if let Err(error) = (DagStaleAlertEvent {
                    report: report.clone(),
                })
                .emit(&self.app)
                {
                    tracing::warn!(%error, "Unable to emit DAG staleness event");
                }
                emit_state(
                    &self.app,
                    "dag_changed",
                    format!("Detected new Git commit {short_sha}"),
                );
                Ok(true)
            }
        }
    }

    async fn sync_client_before_dispatch(
        &self,
        network: &mut Network,
        host: &iroh::EndpointAddr,
    ) -> RelayResult<()> {
        if let Some(sandbox) = &self.sandbox {
            let head = sandbox.head_commit_sha().await?;
            if network.last_pushed_commit.as_deref() != Some(head.as_str()) {
                self.observe_git_head(sandbox, network.local.id().to_string()).await?;
                let snapshot = self.engine.dag_snapshot(true).await?;
                let delta = DagDelta {
                    repo_id: snapshot.repo_id,
                    upsert_nodes: snapshot.nodes,
                    upsert_edges: snapshot.edges,
                };
                let meta = network.local
                    .push_sync_bundle(host.clone(), self.engine.repo_id(), sandbox, delta)
                    .await?;
                network.last_pushed_commit = Some(meta.base_commit_sha);
            }
        }
        if self.engine.pending_outbox_count().await? > 0 {
            let delivered = network.local.flush_outbox(host.clone()).await?;
            if delivered > 0 {
                emit_state(
                    &self.app, "outbox_flushed", format!("Delivered {delivered} queued requests"),
                );
            }
        }
        Ok(())
    }

    fn listen(self: &Arc<Self>, listeners: Vec<NodeEvents>, mode: RuntimeMode) {
        for mut events in listeners {
            let runtime = Arc::downgrade(self);
            let mut shutdown_rx = self.shutdown.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                        maybe_event = events.recv() => {
                            let Some(event) = maybe_event else {
                                break;
                            };
                            let Some(runtime) = runtime.upgrade() else {
                                break;
                            };
                            let execute = mode != RuntimeMode::Client;
                            match event {
                                RelayNodeEvent::TaskSubmitted(id) => runtime.task_submitted(id, execute),
                                RelayNodeEvent::ApprovalResolved(id, patch_id, decision) => {
                                    runtime.approval_resolved(id, patch_id, decision, execute);
                                }
                                RelayNodeEvent::DagChanged => {
                                    emit_state(&runtime.app, "dag_changed", "DAG updated by peer");
                                }
                                RelayNodeEvent::PeerPaired => {
                                    emit_state(&runtime.app, "peer_paired", "Peer connected");
                                }
                            }
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
                let llm_config = runtime.llm_config.read().await.clone();
                emit_state(
                    &runtime.app,
                    "task_preflight_started",
                    format!("Preflight started for task {id}"),
                );
                match run_host_task_pipeline(&runtime.engine, &runtime.mcp, llm_config, id).await {
                    Ok(HostTaskOutcome::Completed | HostTaskOutcome::HealedAndCompleted) => {
                        emit_state(&runtime.app, "task_updated", format!("Task {id} completed"));
                        emit_state(&runtime.app, "dag_changed", "Recorded new run result");
                    }
                    Ok(HostTaskOutcome::PatchProposed) => {
                        emit_state(
                            &runtime.app,
                            "patch_proposed",
                            format!("Verified fix ready for task {id}"),
                        );
                    }
                    Ok(HostTaskOutcome::AwaitingApproval) => {
                        emit_state(
                            &runtime.app, "task_updated", format!("Task {id} is ready for approval"),
                        );
                    }
                    Ok(HostTaskOutcome::Skipped) => {
                        emit_state(&runtime.app, "task_updated", format!("Task {id} skipped"));
                    }
                    Err(error) => {
                        tracing::warn!(task_id = %id, %error, "Task preflight or diagnosis failed");
                        emit_state(
                            &runtime.app,
                            "task_preflight_failed",
                            format!("Task {id}: {error}"),
                        );
                    }
                }
            });
        }
    }

    fn approval_resolved(
        self: &Arc<Self>,
        id: TaskId,
        patch_id: Option<String>,
        decision: ApprovalDecision,
        execute: bool,
    ) {
        emit_state(
            &self.app,
            "approval_resolved",
            format!("Task {id}: {decision:?}"),
        );
        let sync_runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            sync_runtime.sync_chat_approval_card(id, patch_id.as_deref(), decision).await;
        });
        if execute && self.sandbox.is_some() {
            let runtime = self.clone();
            tauri::async_runtime::spawn(async move {
                match runtime.mcp.finish_ui_approval(id, decision).await {
                    Ok(record) => {
                        emit_state(
                            &runtime.app,
                            "task_updated",
                            format!("Task {id}: {:?}", record.state),
                        );
                        emit_state(&runtime.app, "dag_changed", "Task execution finished");
                    }
                    Err(error) => {
                        tracing::warn!(task_id = %id, %error, "Task approval could not finish");
                        emit_state(
                            &runtime.app,
                            "task_execution_failed",
                            format!("Task {id}: {error}"),
                        );
                    }
                }
            });
        }
    }

    fn start_peer_sync(self: &Arc<Self>) {
        let runtime = Arc::downgrade(self);
        let mut shutdown_rx = self.shutdown.subscribe();
        tauri::async_runtime::spawn(async move {
            let mut interval = time::interval(PEER_SYNC_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        let Some(runtime) = runtime.upgrade() else {
                            break;
                        };
                        let Ok(mut guard) = runtime.network.try_lock() else {
                            continue;
                        };
                        let Some(network) = guard.as_mut() else {
                            break;
                        };
                        if network.mode == RuntimeMode::Client && network.remote.is_some() {
                            let status_before = network.local.peer_status();
                            if let Err(error) = runtime.refresh_from_host(network).await {
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
                }
            }
        });
    }

    fn start_git_head_watcher(self: &Arc<Self>) {
        if self.sandbox.is_none() {
            return;
        }
        let runtime = Arc::downgrade(self);
        let mut shutdown_rx = self.shutdown.subscribe();
        tauri::async_runtime::spawn(async move {
            let mut interval = time::interval(GIT_WATCH_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        let Some(runtime) = runtime.upgrade() else {
                            break;
                        };
                        if let Err(error) = runtime.check_git_head().await {
                            tracing::debug!(%error, "Git HEAD check failed");
                        }
                    }
                }
            }
        });
    }

    async fn refresh_from_host(&self, network: &mut Network) -> RelayResult<()> {
        let Some(address) = network.remote.clone() else {
            return Ok(());
        };
        self.sync_client_before_dispatch(network, &address).await?;
        let snapshot = fetch_dag(&network.local, address.clone(), self.engine.repo_id()).await?;
        let current = self.engine.dag_snapshot(true).await?;
        if current.nodes != snapshot.nodes || current.edges != snapshot.edges {
            let merged = self.engine.apply_dag_delta(DagDelta {
                repo_id: snapshot.repo_id.clone(),
                upsert_nodes: snapshot.nodes.clone(),
                upsert_edges: snapshot.edges.clone(),
            }).await?;
            if merged.nodes != current.nodes || merged.edges != current.edges {
                emit_state(&self.app, "dag_changed", "DAG refreshed from host");
            }
            if merged.nodes != snapshot.nodes || merged.edges != snapshot.edges {
                let request = WireRequest::SyncDagDelta {
                    delta: DagDelta {
                        repo_id: merged.repo_id,
                        upsert_nodes: merged.nodes,
                        upsert_edges: merged.edges,
                    },
                };
                match network.local.send_rpc(address.clone(), request).await? {
                    WireResponse::DagSnapshot(_) => {}
                    response => return Err(rpc_error(response, "DAG snapshot")),
                }
            }
        }
        let mut changed = false;
        for task in self.engine.list_tasks().await? {
            let request = WireRequest::FetchTaskState {
                task_id: task.spec.task_id,
            };
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

pub(crate) fn validate_user_repo(path: &Path) -> RelayResult<PathBuf> {
    let abs = std::path::absolute(path)?;
    if !abs.is_dir() {
        return Err(RelayError::Internal(format!(
            "Folder does not exist: {}",
            abs.display()
        )));
    }
    if !abs.join(".git").exists() {
        return Err(RelayError::Internal(
            "Selected folder is not a Git repository (missing .git). Run `git init` first.".into(),
        ));
    }
    if abs.join("relay.toml").is_file() {
        let _ = load_manifest(&abs)?;
    }
    Ok(abs)
}

fn inspect_workspace(repo_root: Option<&Path>) -> WorkspaceInfo {
    let Some(root) = repo_root else {
        return WorkspaceInfo {
            repo_id: DEFAULT_REPO_ID.into(),
            name: String::new(),
            repo_root: None,
            mcp_data_dir: None,
            is_git_repo: false,
            has_manifest: false,
            smoke_preset: String::new(),
            presets: Vec::new(),
        };
    };
    let is_git_repo = root.join(".git").exists();
    match load_manifest(root) {
        Ok(manifest) => {
            let presets = manifest
                .presets
                .into_iter()
                .map(|(name, preset)| PresetInfo {
                    name,
                    description: preset.description,
                    program: preset.program,
                    args: preset.args,
                    allowed_params: preset.allowed_params,
                })
                .collect();
            WorkspaceInfo {
                repo_id: manifest.project.repo_id,
                name: manifest.project.name,
                repo_root: Some(root.to_string_lossy().into_owned()),
                mcp_data_dir: None,
                is_git_repo,
                has_manifest: true,
                smoke_preset: manifest.project.smoke_preset,
                presets,
            }
        }
        Err(_) => {
            let fallback_name = root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| DEFAULT_REPO_ID.into());
            WorkspaceInfo {
                repo_id: relay_core::sandbox::slugify_repo_id(&fallback_name),
                name: fallback_name,
                repo_root: Some(root.to_string_lossy().into_owned()),
                mcp_data_dir: None,
                is_git_repo,
                has_manifest: root.join("relay.toml").exists(),
                smoke_preset: String::new(),
                presets: Vec::new(),
            }
        }
    }
}

fn load_workspace_prefs(base_data_dir: &Path) -> WorkspacePrefs {
    let path = base_data_dir.join("workspace.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn sanitize_repo_dir(repo_id: &str) -> String {
    let cleaned: String = repo_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        DEFAULT_REPO_ID.into()
    } else {
        cleaned
    }
}

async fn fallback_artifact(
    engine: &RelayEngine,
    old_artifact_id: &str,
    new_commit_sha: String,
    device_origin: String,
) -> RelayResult<GraphNode> {
    let snapshot = engine.dag_snapshot(true).await?;
    let fingerprint = snapshot.nodes.into_iter().find_map(|node| match node.payload {
        NodeKind::Artifact {
            env_fingerprint, ..
        } if node.id == old_artifact_id => Some(env_fingerprint),
        _ => None,
    });
    let fingerprint =
        fingerprint.ok_or_else(|| RelayError::NodeNotFound(old_artifact_id.into()))?;
    let now = Utc::now();
    let short_sha: String = new_commit_sha.chars().take(7).collect();
    Ok(GraphNode {
        id: format!("artifact:{new_commit_sha}"),
        label: format!("Commit {short_sha}"),
        payload: NodeKind::Artifact {
            git_commit_sha: new_commit_sha,
            env_fingerprint: fingerprint,
            working_tree_dirty: false,
            changed_files: Vec::new(),
        },
        freshness: Freshness::Fresh,
        device_origin,
        created_at: now,
        updated_at: now,
    })
}

fn task_response(response: WireResponse) -> RelayResult<TaskRecord> {
    match response {
        WireResponse::TaskState(record) => Ok(record),
        response => Err(rpc_error(response, "task state")),
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn same_directory(left: &Path, right: &Path) -> bool {
    left == right || matches!((left.canonicalize(), right.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

async fn resolve_base_data_dir(app: &AppHandle) -> RelayResult<PathBuf> {
    let dir = if let Some(dir) = env_path("RELAY_DATA_DIR") {
        dir
    } else if let Ok(local_dir) = app.path().app_local_data_dir() {
        local_dir.join("relay-desktop")
    } else {
        env::temp_dir().join("relay-desktop")
    };
    let dir = std::path::absolute(dir)?;
    tokio::fs::create_dir_all(&dir).await?;
    Ok(dir)
}
