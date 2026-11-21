use chrono::Utc;
use relay_core::contracts::{
    ApprovalDecision, DagSnapshot, Freshness, GraphEdge, GraphNode, HandoffSummary, NodeKind,
    PeerStatus, RuntimeMode, StalePropagationReport, TaskRecord, TaskSpec, TaskState,
    TransportPath,
};
use relay_core::dag::DagEngine;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Typescript;
use std::path::Path;
use std::sync::Arc;
use tauri::State;
use tauri_specta::{Builder, Event, collect_commands, collect_events};
use tokio::sync::RwLock;
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

pub struct AppState {
    pub mode: RwLock<RuntimeMode>,
    pub dag: Arc<RwLock<DagEngine>>,
    pub tasks: RwLock<Vec<TaskRecord>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            mode: RwLock::new(RuntimeMode::LocalDualSandbox),
            dag: Arc::new(RwLock::new(DagEngine::new("forecast-bench"))),
            tasks: RwLock::new(Vec::new()),
        }
    }
}

#[tauri::command]
#[specta::specta]
async fn get_runtime_status(state: State<'_, AppState>) -> Result<PeerStatus, String> {
    let mode = *state.mode.read().await;
    let snap = state.dag.read().await.snapshot(true);
    Ok(PeerStatus {
        mode,
        local_node_id: "pending-iroh-node-id".into(),
        remote_node_id: None,
        transport_path: match mode {
            RuntimeMode::LocalDualSandbox => TransportPath::Loopback,
            _ => TransportPath::Offline,
        },
        connected: matches!(mode, RuntimeMode::LocalDualSandbox),
        rtt_ms: Some(1),
        last_synced_commit: snap.head_commit_sha,
        pending_outbox_count: 0,
    })
}

#[tauri::command]
#[specta::specta]
async fn switch_runtime_mode(
    state: State<'_, AppState>,
    mode: RuntimeMode,
) -> Result<RuntimeMode, String> {
    let mut guard = state.mode.write().await;
    *guard = mode;
    Ok(mode)
}

#[tauri::command]
#[specta::specta]
async fn get_dag_snapshot(
    state: State<'_, AppState>,
    include_stale: bool,
) -> Result<DagSnapshot, String> {
    Ok(state.dag.read().await.snapshot(include_stale))
}

#[tauri::command]
#[specta::specta]
async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<TaskRecord>, String> {
    Ok(state.tasks.read().await.clone())
}

#[tauri::command]
#[specta::specta]
async fn list_handoffs(_state: State<'_, AppState>) -> Result<Vec<HandoffSummary>, String> {
    Ok(Vec::new())
}

#[tauri::command]
#[specta::specta]
async fn submit_task(
    state: State<'_, AppState>,
    mut spec: TaskSpec,
) -> Result<TaskRecord, String> {
    if spec.task_id.is_nil() {
        spec.task_id = Uuid::now_v7();
    }
    let now = Utc::now();
    let record = TaskRecord {
        spec,
        state: TaskState::Queued,
        active_proposal: None,
        preflight_summary: None,
        evidence_node_id: None,
        error_message: None,
        created_at: now,
        updated_at: now,
    };
    state.tasks.write().await.push(record.clone());
    Ok(record)
}

#[tauri::command]
#[specta::specta]
async fn resolve_patch_approval(
    state: State<'_, AppState>,
    task_id: Uuid,
    decision: ApprovalDecision,
) -> Result<TaskRecord, String> {
    let mut tasks = state.tasks.write().await;
    let task = tasks
        .iter_mut()
        .find(|t| t.spec.task_id == task_id)
        .ok_or_else(|| format!("task not found: {task_id}"))?;
    task.state = match decision {
        ApprovalDecision::Approved => TaskState::Executing,
        ApprovalDecision::Rejected => TaskState::Rejected,
    };
    task.updated_at = Utc::now();
    Ok(task.clone())
}

#[tauri::command]
#[specta::specta]
async fn simulate_artifact_update(
    state: State<'_, AppState>,
    old_artifact_id: String,
    new_commit_sha: String,
    reason: String,
) -> Result<StalePropagationReport, String> {
    let now = Utc::now();
    let new_node = GraphNode {
        id: format!("artifact:{new_commit_sha}"),
        label: format!("Commit {}", &new_commit_sha[..new_commit_sha.len().min(7)]),
        payload: NodeKind::Artifact {
            git_commit_sha: new_commit_sha,
            env_fingerprint: "uv-lock-v1".into(),
            working_tree_dirty: false,
            changed_files: vec!["src/forecast.py".into()],
        },
        freshness: Freshness::Fresh,
        device_origin: "laptop-client".into(),
        created_at: now,
        updated_at: now,
    };
    let mut dag = state.dag.write().await;
    dag.supersede_artifact(&old_artifact_id, new_node, reason)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
async fn upsert_dag_node(
    state: State<'_, AppState>,
    node: GraphNode,
    edges: Vec<GraphEdge>,
) -> Result<DagSnapshot, String> {
    let mut dag = state.dag.write().await;
    dag.upsert_node(node);
    for edge in edges {
        dag.add_edge(edge).map_err(|e| e.to_string())?;
    }
    Ok(dag.snapshot(true))
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
        ])
        .events(collect_events![RelayStateChangedEvent, DagStaleAlertEvent])
}

pub fn export_bindings_to(path: &Path) -> Result<(), String> {
    specta_builder()
        .export(Typescript::default(), path)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // Only auto-export bindings in debug mode when frontend `../src` already exists
    // (respecting custom frontend scaffolding via create app).
    #[cfg(debug_assertions)]
    {
        let src_dir = Path::new("../src");
        if src_dir.exists() {
            builder
                .export(Typescript::default(), src_dir.join("bindings.ts"))
                .expect("failed to export TypeScript bindings via tauri-specta");
        }
    }

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Relay desktop application");
}
