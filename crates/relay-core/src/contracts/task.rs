use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;
use uuid::Uuid;

pub type TaskId = Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMode {
    Client,
    Host,
    LocalDualSandbox,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TransportPath {
    DirectLan,
    Relay,
    Loopback,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct PeerStatus {
    pub mode: RuntimeMode,
    pub local_node_id: String,
    pub remote_node_id: Option<String>,
    pub transport_path: TransportPath,
    pub connected: bool,
    pub rtt_ms: Option<u32>,
    pub last_synced_commit: Option<String>,
    pub pending_outbox_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct TaskSpec {
    pub task_id: TaskId,
    pub repo_id: String,
    pub base_commit_sha: String,
    pub command_preset: String,
    pub params: BTreeMap<String, String>,
    pub require_approval: bool,
    pub requested_by_device: String,
    pub user_note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    PreflightRunning,
    SelfHealingProposed,
    Executing,
    Completed,
    Failed,
    Stale,
    Rejected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PatchKind {
    DependencySpec,
    CodeDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type, JsonSchema)]
pub struct FileEdit {
    #[schemars(description = "Worktree-relative path of the file to edit or create (e.g. 'src/app.py')")]
    pub file_path: String,
    #[schemars(description = "Exact text snippet to replace (must match uniquely); omit or set null to overwrite or create the entire file")]
    #[serde(default)]
    pub old_text: Option<String>,
    #[schemars(description = "Replacement text (or full file contents when old_text is omitted/null)")]
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct PatchProposal {
    pub patch_id: String,
    pub task_id: TaskId,
    pub kind: PatchKind,
    pub summary: String,
    pub diff_preview: String,
    pub worktree_branch: String,
    pub smoke_verified: bool,
    pub smoke_stdout_tail: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct TaskRecord {
    pub spec: TaskSpec,
    pub state: TaskState,
    pub active_proposal: Option<PatchProposal>,
    pub preflight_summary: Option<String>,
    pub evidence_node_id: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct HandoffSummary {
    pub handoff_id: String,
    pub repo_id: String,
    pub source_device: String,
    pub base_commit_sha: String,
    pub summary: String,
    pub active_task_count: u32,
    pub stale_evidence_count: u32,
    pub created_at: DateTime<Utc>,
}

/// Project-level command whitelist loaded from `<repo>/relay.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct RelayManifest {
    pub project: ProjectMeta,
    #[serde(default)]
    pub presets: BTreeMap<String, CommandPreset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct ProjectMeta {
    pub repo_id: String,
    pub name: String,
    #[serde(default = "default_smoke_preset")]
    pub smoke_preset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<String>,
}

fn default_smoke_preset() -> String {
    "smoke".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct CommandPreset {
    pub description: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u32,
    #[serde(default)]
    pub allowed_params: Vec<String>,
}

fn default_timeout_secs() -> u32 {
    300
}
