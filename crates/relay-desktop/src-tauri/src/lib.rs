mod runtime;

use chrono::Utc;
use relay_core::agent::{
    AgentApprovalCard, AgentChatMessage, AgentToolTrace, InferenceTier, LlmProbeResult,
    LlmSettingsView,
};
use relay_core::contracts::{
    ApprovalDecision, DagSnapshot, GraphEdge, GraphNode, HandoffSummary, PeerStatus, RuntimeMode,
    StalePropagationReport, TaskRecord, TaskSpec,
};
use runtime::{DesktopRuntime, validate_user_repo};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tauri_specta::{Builder, Event, collect_commands, collect_events};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, Type, Event)]
pub struct RelayStateChangedEvent {
    pub kind: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, Event)]
pub struct DagStaleAlertEvent {
    pub report: StalePropagationReport,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, Event)]
pub struct AgentChatStreamEvent {
    pub turn_id: String,
    pub message_id: String,
    pub kind: String,
    pub delta: Option<String>,
    pub trace: Option<AgentToolTrace>,
    pub approval: Option<AgentApprovalCard>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PresetInfo {
    pub name: String,
    pub description: String,
    pub program: String,
    pub args: Vec<String>,
    pub allowed_params: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct WorkspaceInfo {
    pub repo_id: String,
    pub name: String,
    pub repo_root: Option<String>,
    pub mcp_data_dir: Option<String>,
    pub is_git_repo: bool,
    pub has_manifest: bool,
    pub smoke_preset: String,
    pub presets: Vec<PresetInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct LlmSettingsUpdate {
    pub tier: InferenceTier,
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
    /// `None` keeps any stored key; `Some("")` clears it; `Some(key)` replaces it.
    pub api_key: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Default)]
struct RuntimeManager {
    current: RwLock<Option<Arc<DesktopRuntime>>>,
    lifecycle: Mutex<()>,
}

impl RuntimeManager {
    async fn get_or_init(&self, app: &AppHandle) -> Result<Arc<DesktopRuntime>, String> {
        if let Some(runtime) = self.current.read().await.clone() {
            return Ok(runtime);
        }
        let _guard = self.lifecycle.lock().await;
        if let Some(runtime) = self.current.read().await.clone() {
            return Ok(runtime);
        }
        let opened = DesktopRuntime::open(app.clone(), None)
            .await
            .map_err(|error| error.to_string())?;
        *self.current.write().await = Some(opened.clone());
        Ok(opened)
    }

    async fn switch_repo(
        &self,
        app: &AppHandle,
        repo_path: String,
    ) -> Result<WorkspaceInfo, String> {
        let validated =
            validate_user_repo(&PathBuf::from(repo_path.trim())).map_err(|error| error.to_string())?;
        let _guard = self.lifecycle.lock().await;
        let mut current = self.current.write().await;
        if let Some(active) = current.as_ref() && active.matches_workspace(&validated) {
            return Ok(active.workspace_info());
        }
        let shares_storage = current.as_ref().is_some_and(|runtime| runtime.shares_storage(&validated));
        if shares_storage && let Some(previous) = current.take() {
            // iroh-blobs holds an exclusive lock until the old router finishes shutting down.
            previous.shutdown().await;
        }
        let next = DesktopRuntime::open(app.clone(), Some(validated.clone()))
            .await
            .map_err(|error| error.to_string())?;
        if let Err(error) = next.persist_repo_preference(&validated) {
            tracing::warn!(%error, "Unable to persist selected workspace repository");
        }
        let info = next.workspace_info();
        let previous = current.replace(next);
        drop(current);
        if let Some(previous) = previous {
            previous.shutdown().await;
        }
        emit_state(
            app,
            "workspace_changed",
            format!("Opened repository {}", info.repo_id),
        );
        Ok(info)
    }
}

fn emit_state(app: &AppHandle, kind: &str, message: impl Into<String>) {
    let event = RelayStateChangedEvent {
        kind: kind.into(),
        message: message.into(),
        timestamp: Utc::now().to_rfc3339(),
    };
    if let Err(error) = event.emit(app) {
        tracing::warn!(%error, kind, "Unable to emit Relay state change");
    }
}

#[tauri::command]
#[specta::specta]
async fn get_runtime_status(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<PeerStatus, String> {
    Ok(state.get_or_init(&app).await?.status().await)
}

#[tauri::command]
#[specta::specta]
async fn get_workspace_info(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<WorkspaceInfo, String> {
    Ok(state.get_or_init(&app).await?.workspace_info())
}

#[tauri::command]
#[specta::specta]
async fn pick_repository_folder() -> Result<Option<String>, String> {
    let folder = rfd::AsyncFileDialog::new()
        .set_title("Select Local Git Repository")
        .pick_folder()
        .await;
    Ok(folder.map(|handle| handle.path().to_string_lossy().into_owned()))
}

#[tauri::command]
#[specta::specta]
async fn open_workspace_repo(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    repo_path: String,
) -> Result<WorkspaceInfo, String> {
    state.switch_repo(&app, repo_path).await
}

#[tauri::command]
#[specta::specta]
async fn init_workspace_manifest(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    repo_path: Option<String>,
) -> Result<WorkspaceInfo, String> {
    let target = match repo_path
        .map(|raw| raw.trim().to_owned())
        .filter(|raw| !raw.is_empty())
    {
        Some(raw) => PathBuf::from(raw),
        None => {
            let runtime = state.get_or_init(&app).await?;
            let root = runtime
                .workspace_info()
                .repo_root
                .ok_or_else(|| "Open a local Git repository first.".to_owned())?;
            PathBuf::from(root)
        }
    };
    let validated = validate_user_repo(&target).map_err(|error| error.to_string())?;
    relay_core::sandbox::write_default_manifest(&validated).map_err(|error| error.to_string())?;
    state
        .switch_repo(&app, validated.to_string_lossy().into_owned())
        .await
}

#[tauri::command]
#[specta::specta]
async fn refresh_git_head(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<bool, String> {
    state
        .get_or_init(&app)
        .await?
        .check_git_head()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn get_llm_settings(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<LlmSettingsView, String> {
    Ok(state.get_or_init(&app).await?.llm_settings_view().await)
}

#[tauri::command]
#[specta::specta]
async fn save_llm_settings(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    update: LlmSettingsUpdate,
) -> Result<LlmSettingsView, String> {
    state
        .get_or_init(&app)
        .await?
        .save_llm_settings(update)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn test_llm_connection(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    update: Option<LlmSettingsUpdate>,
) -> Result<LlmProbeResult, String> {
    state
        .get_or_init(&app)
        .await?
        .test_llm_connection(update)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn switch_runtime_mode(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    mode: RuntimeMode,
) -> Result<RuntimeMode, String> {
    state
        .get_or_init(&app)
        .await?
        .switch_mode(mode)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn get_dag_snapshot(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    include_stale: bool,
) -> Result<DagSnapshot, String> {
    let runtime = state.get_or_init(&app).await?;
    runtime.engine.dag_snapshot(include_stale).await.map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn list_tasks(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<Vec<TaskRecord>, String> {
    state
        .get_or_init(&app)
        .await?
        .engine
        .list_tasks()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn list_handoffs(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<Vec<HandoffSummary>, String> {
    state
        .get_or_init(&app)
        .await?
        .engine
        .list_handoffs(u32::MAX)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn submit_task(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    spec: TaskSpec,
) -> Result<TaskRecord, String> {
    state
        .get_or_init(&app)
        .await?
        .submit(spec)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn resolve_patch_approval(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    task_id: Uuid,
    patch_id: Option<String>,
    decision: ApprovalDecision,
) -> Result<TaskRecord, String> {
    state
        .get_or_init(&app)
        .await?
        .resolve_approval(task_id, patch_id, decision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn simulate_artifact_update(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    old_artifact_id: String,
    new_commit_sha: String,
    reason: String,
) -> Result<StalePropagationReport, String> {
    state
        .get_or_init(&app)
        .await?
        .supersede_artifact(&old_artifact_id, new_commit_sha, reason)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn upsert_dag_node(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    node: GraphNode,
    edges: Vec<GraphEdge>,
) -> Result<DagSnapshot, String> {
    let snapshot = state
        .get_or_init(&app)
        .await?
        .engine
        .upsert_node_and_edges(node, edges)
        .await
        .map_err(|error| error.to_string())?;
    emit_state(&app, "dag_changed", "DAG node updated");
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
async fn get_pairing_ticket(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<String, String> {
    state
        .get_or_init(&app)
        .await?
        .pairing_ticket()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn connect_peer_ticket(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    ticket_json: String,
) -> Result<PeerStatus, String> {
    state
        .get_or_init(&app)
        .await?
        .connect_ticket(&ticket_json)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn list_chat_messages(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    limit: Option<u32>,
) -> Result<Vec<AgentChatMessage>, String> {
    state
        .get_or_init(&app)
        .await?
        .list_chat_messages(limit)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn send_chat_message(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    prompt: String,
) -> Result<AgentChatMessage, String> {
    state
        .get_or_init(&app)
        .await?
        .send_chat_message(prompt)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn cancel_chat_turn(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<bool, String> {
    Ok(state.get_or_init(&app).await?.cancel_chat_turn().await)
}

#[tauri::command]
#[specta::specta]
async fn clear_chat_messages(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
) -> Result<(), String> {
    state
        .get_or_init(&app)
        .await?
        .clear_chat_messages()
        .await
        .map_err(|error| error.to_string())
}

pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            get_runtime_status,
            get_workspace_info,
            pick_repository_folder,
            open_workspace_repo,
            init_workspace_manifest,
            refresh_git_head,
            get_llm_settings,
            save_llm_settings,
            test_llm_connection,
            switch_runtime_mode,
            get_dag_snapshot,
            list_tasks,
            list_handoffs,
            submit_task,
            resolve_patch_approval,
            simulate_artifact_update,
            upsert_dag_node,
            get_pairing_ticket,
            connect_peer_ticket,
            list_chat_messages,
            send_chat_message,
            cancel_chat_turn,
            clear_chat_messages,
        ])
        .events(collect_events![
            RelayStateChangedEvent,
            DagStaleAlertEvent,
            AgentChatStreamEvent,
        ])
}

pub fn export_bindings_to(path: &Path) -> Result<(), String> {
    specta_builder()
        .export(Typescript::default(), path)
        .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    tauri::Builder::default()
        .manage(RuntimeManager::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<RuntimeManager>();
                match state.get_or_init(&app).await {
                    Ok(_) => emit_state(&app, "runtime_ready", "Relay runtime started"),
                    Err(error) => {
                        tracing::error!(%error, "Unable to start Relay runtime");
                        emit_state(&app, "runtime_start_failed", error);
                    }
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Relay desktop application");
}
