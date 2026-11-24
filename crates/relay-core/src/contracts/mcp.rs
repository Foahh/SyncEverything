use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListHandoffsArgs {
    #[schemars(description = "Maximum number of recent handoffs to return (default 10)")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckEnvDiffArgs {
    #[schemars(description = "Target Git commit SHA to compare against the Host environment")]
    pub commit_sha: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GetDagSnapshotArgs {
    #[schemars(description = "Whether to include STALE evidence nodes (still flagged as STALE)")]
    pub include_stale: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadEvidenceLogArgs {
    #[schemars(description = "DAG Evidence node ID to inspect")]
    pub node_id: String,
    #[schemars(description = "Maximum tail lines of stdout/stderr log to return (default 120)")]
    pub max_lines: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunPreflightArgs {
    #[schemars(description = "UUIDv7 of the queued task to preflight in an isolated git worktree")]
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposeWorktreePatchArgs {
    #[schemars(description = "UUIDv7 of the task undergoing self-healing")]
    pub task_id: String,
    #[schemars(description = "Patch type: 'uv_spec' for dependency fix or 'code_diff' for unified diff")]
    pub patch_kind: String,
    #[schemars(description = "Short human-readable explanation of the root cause and fix")]
    pub summary: String,
    #[schemars(description = "Package spec (e.g. 'requests>=2.32') or unified git diff patch content")]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApproveAndExecuteArgs {
    #[schemars(description = "UUIDv7 of the task to execute")]
    pub task_id: String,
    #[schemars(description = "Optional verified patch_id to apply before running the experiment")]
    pub patch_id: Option<String>,
    #[schemars(description = "Set true only when approval has already been recorded via UI or Elicitation")]
    pub approved_via_ui: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ApprovalExecutionResponse {
    ApprovalRequired {
        task_id: String,
        patch_id: Option<String>,
        summary: String,
        diff_preview: String,
    },
    ExecutionStarted {
        task_id: String,
        worktree_branch: String,
    },
    Rejected {
        task_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct EnvDiffReport {
    pub commit_sha: String,
    pub repo_env_fingerprint: String,
    pub host_env_fingerprint: String,
    pub in_sync: bool,
    pub missing_packages: Vec<String>,
    pub notes: Vec<String>,
}
