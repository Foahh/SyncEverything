use crate::contracts::FileEdit;
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
#[serde(deny_unknown_fields)]
pub struct GetDagSnapshotArgs {}

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
pub struct ReadWorktreeFileArgs {
    #[schemars(description = "UUIDv7 of the task whose isolated worktree should be read")]
    pub task_id: String,
    #[schemars(description = "Worktree-relative path to a UTF-8 file (e.g. 'src/main.py' or 'uv.lock')")]
    pub file_path: String,
    #[schemars(description = "Optional 1-based inclusive start line (default 1)")]
    #[serde(default)]
    pub start_line: Option<u32>,
    #[schemars(description = "Optional 1-based inclusive end line (default 200)")]
    #[serde(default)]
    pub end_line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeFileView {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub truncated: bool,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposeWorktreePatchArgs {
    #[schemars(description = "UUIDv7 of the task undergoing self-healing")]
    pub task_id: String,
    #[schemars(description = "Patch type: 'uv_spec' for dependency fix or 'file_edits' for exact search-and-replace / full-file edits")]
    pub patch_kind: String,
    #[schemars(description = "Short human-readable explanation of the root cause and fix")]
    pub summary: String,
    #[schemars(description = "Required when patch_kind is 'uv_spec': a single PEP-508 package spec or local wheel path (e.g. 'requests>=2.32' or './vendor/pkg.whl')")]
    #[serde(default)]
    pub uv_spec: Option<String>,
    #[schemars(description = "Required when patch_kind is 'file_edits': ordered list of file edits applied atomically in the task worktree")]
    #[serde(default)]
    pub edits: Option<Vec<FileEdit>>,
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
