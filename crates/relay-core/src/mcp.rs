use crate::contracts::{
    ApprovalExecutionResponse, ApproveAndExecuteArgs, CheckEnvDiffArgs, EnvDiffReport,
    GetDagSnapshotArgs, ListHandoffsArgs, ProposeWorktreePatchArgs, ReadEvidenceLogArgs,
    RunPreflightArgs,
};
use crate::dag::DagEngine;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared state backing all frozen MCP tools (accessible over Stdio & Streamable HTTP).
#[derive(Clone)]
pub struct RelayMcpServer {
    dag: Arc<RwLock<DagEngine>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl RelayMcpServer {
    pub fn new(dag: Arc<RwLock<DagEngine>>) -> Self {
        Self {
            dag,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Inspect cross-device P2P sync status, active transport path, and pending outbox count.")]
    async fn inspect_sync_status(&self) -> Result<CallToolResult, McpError> {
        let payload = serde_json::json!({
            "status": "skeleton",
            "transport_path": "loopback",
            "pending_outbox_count": 0
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            payload.to_string(),
        )]))
    }

    #[tool(description = "List recent cross-device handoffs with commit SHA and active task counts.")]
    async fn list_handoffs(
        &self,
        Parameters(args): Parameters<ListHandoffsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let limit = args.limit.unwrap_or(10);
        let payload = serde_json::json!({ "limit": limit, "handoffs": [] });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            payload.to_string(),
        )]))
    }

    #[tool(description = "Compare target commit uv.lock/pyproject environment fingerprint against the Host.")]
    async fn check_env_diff(
        &self,
        Parameters(args): Parameters<CheckEnvDiffArgs>,
    ) -> Result<CallToolResult, McpError> {
        let report = EnvDiffReport {
            commit_sha: args.commit_sha,
            repo_env_fingerprint: "pending".into(),
            host_env_fingerprint: "pending".into(),
            in_sync: true,
            missing_packages: vec![],
            notes: vec!["Environment checker skeleton ready".into()],
        };
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Fetch the Decision-Evidence DAG snapshot. STALE evidence nodes are explicitly marked and must never be cited as valid.")]
    async fn get_dag_snapshot(
        &self,
        Parameters(args): Parameters<GetDagSnapshotArgs>,
    ) -> Result<CallToolResult, McpError> {
        let include_stale = args.include_stale.unwrap_or(true);
        let snap = self.dag.read().await.snapshot(include_stale);
        let text = serde_json::to_string_pretty(&snap)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Read metrics and log tail for a specific Evidence DAG node, blocking or warning if STALE.")]
    async fn read_evidence_log(
        &self,
        Parameters(args): Parameters<ReadEvidenceLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        let dag = self.dag.read().await;
        match dag.assert_evidence_fresh(&args.node_id) {
            Ok(node) => {
                let text = serde_json::to_string_pretty(node)
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
            }
            Err(err) => Ok(CallToolResult::error(vec![ContentBlock::text(err.to_string())])),
        }
    }

    #[tool(description = "Run isolated git worktree + uv smoke preflight for a queued task without touching the main working tree.")]
    async fn run_preflight_in_worktree(
        &self,
        Parameters(args): Parameters<RunPreflightArgs>,
    ) -> Result<CallToolResult, McpError> {
        let payload = serde_json::json!({
            "task_id": args.task_id,
            "status": "preflight_skeleton"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            payload.to_string(),
        )]))
    }

    #[tool(description = "Apply a candidate uv_spec or code_diff patch inside the isolated worktree, verify smoke pass, and persist a PatchProposal.")]
    async fn propose_worktree_patch(
        &self,
        Parameters(args): Parameters<ProposeWorktreePatchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let payload = serde_json::json!({
            "task_id": args.task_id,
            "patch_kind": args.patch_kind,
            "summary": args.summary,
            "state": "self_healing_proposed"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            payload.to_string(),
        )]))
    }

    #[tool(description = "Approve a verified self-healing patch (via MCP Elicitation or UI approval) and launch the whitelisted experiment preset.")]
    async fn approve_and_execute_task(
        &self,
        Parameters(args): Parameters<ApproveAndExecuteArgs>,
    ) -> Result<CallToolResult, McpError> {
        let resp = if args.approved_via_ui.unwrap_or(false) {
            ApprovalExecutionResponse::ExecutionStarted {
                task_id: args.task_id,
                worktree_branch: "relay/worktree/pending".into(),
            }
        } else {
            ApprovalExecutionResponse::ApprovalRequired {
                task_id: args.task_id,
                patch_id: args.patch_id,
                summary: "Human approval required before applying patch & running preset".into(),
                diff_preview: String::new(),
            }
        };
        let text = serde_json::to_string_pretty(&resp)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RelayMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
