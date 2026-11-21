mod runtime;

use chrono::Utc;
use relay_core::contracts::{
    ApprovalDecision, DagSnapshot, GraphEdge, GraphNode, HandoffSummary, PeerStatus, RuntimeMode,
    StalePropagationReport, TaskRecord, TaskSpec,
};
use runtime::DesktopRuntime;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tauri_specta::{Builder, Event, collect_commands, collect_events};
use tokio::sync::OnceCell;
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

type RuntimeState = OnceCell<Arc<DesktopRuntime>>;

async fn live_runtime<'a>(
    app: &AppHandle,
    state: &'a RuntimeState,
) -> Result<&'a Arc<DesktopRuntime>, String> {
    state
        .get_or_try_init(|| DesktopRuntime::open(app.clone()))
        .await
        .map_err(|error| error.to_string())
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
    state: State<'_, RuntimeState>,
) -> Result<PeerStatus, String> {
    Ok(live_runtime(&app, &state).await?.status().await)
}

#[tauri::command]
#[specta::specta]
async fn switch_runtime_mode(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    mode: RuntimeMode,
) -> Result<RuntimeMode, String> {
    live_runtime(&app, &state)
        .await?
        .switch_mode(mode)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn get_dag_snapshot(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    include_stale: bool,
) -> Result<DagSnapshot, String> {
    let runtime = live_runtime(&app, &state).await?;
    Ok(runtime.engine.dag_snapshot(include_stale).await)
}

#[tauri::command]
#[specta::specta]
async fn list_tasks(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<Vec<TaskRecord>, String> {
    live_runtime(&app, &state)
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
    state: State<'_, RuntimeState>,
) -> Result<Vec<HandoffSummary>, String> {
    live_runtime(&app, &state)
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
    state: State<'_, RuntimeState>,
    spec: TaskSpec,
) -> Result<TaskRecord, String> {
    live_runtime(&app, &state)
        .await?
        .submit(spec)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn resolve_patch_approval(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    task_id: Uuid,
    decision: ApprovalDecision,
) -> Result<TaskRecord, String> {
    live_runtime(&app, &state)
        .await?
        .resolve_approval(task_id, decision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn simulate_artifact_update(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    old_artifact_id: String,
    new_commit_sha: String,
    reason: String,
) -> Result<StalePropagationReport, String> {
    live_runtime(&app, &state)
        .await?
        .supersede_artifact(&old_artifact_id, new_commit_sha, reason)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn upsert_dag_node(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    node: GraphNode,
    edges: Vec<GraphEdge>,
) -> Result<DagSnapshot, String> {
    let snapshot = live_runtime(&app, &state)
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
    state: State<'_, RuntimeState>,
) -> Result<String, String> {
    live_runtime(&app, &state)
        .await?
        .pairing_ticket()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
async fn connect_peer_ticket(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    ticket_json: String,
) -> Result<PeerStatus, String> {
    live_runtime(&app, &state)
        .await?
        .connect_ticket(&ticket_json)
        .await
        .map_err(|error| error.to_string())
}

pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            get_runtime_status,
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
        ])
        .events(collect_events![RelayStateChangedEvent, DagStaleAlertEvent])
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
        .manage(RuntimeState::new())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = app.state::<RuntimeState>();
                match live_runtime(&app, &state).await {
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
