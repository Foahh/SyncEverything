use crate::contracts::{
    ApprovalDecision, ApprovalExecutionResponse, ApproveAndExecuteArgs, CheckEnvDiffArgs,
    DagSnapshot, EnvDiffReport, Freshness, GetDagSnapshotArgs, GraphNode, HandoffSummary, JsonValue,
    ListHandoffsArgs, NodeKind, PatchKind, PatchProposal, PeerStatus, ProposeWorktreePatchArgs,
    ReadEvidenceLogArgs, RunPreflightArgs, TaskId, TaskRecord, TaskState, TransportPath,
};
use crate::engine::RelayEngine;
use crate::error::{RelayError, RelayResult};
use crate::sandbox::{PreflightReport, WorktreeHandle, WorktreeSandbox, load_manifest};
use crate::state::validate_transition;
use crate::sync::RelayNode;
use chrono::Utc;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::service::{ElicitationError, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedMutexGuard};
use uuid::Uuid;

struct RelayMcpContext {
    engine: Arc<RelayEngine>,
    sandbox: Option<WorktreeSandbox>,
    node: Option<Arc<RelayNode>>,
    active_worktrees: Arc<Mutex<HashMap<TaskId, WorktreeHandle>>>,
    task_locks: Mutex<HashMap<TaskId, Weak<Mutex<()>>>>,
}

#[derive(Clone)]
pub struct RelayMcpServer {
    context: Arc<RelayMcpContext>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ApprovalForm {
    pub approve: bool,
    pub note: Option<String>,
}

rmcp::elicit_safe!(ApprovalForm);

#[derive(Debug, Serialize, Deserialize)]
pub struct EvidenceLog {
    pub node: GraphNode,
    pub log_tail: String,
}

impl RelayMcpServer {
    pub fn new(
        engine: Arc<RelayEngine>,
        sandbox: Option<WorktreeSandbox>,
        node: Option<Arc<RelayNode>>,
    ) -> Self {
        Self {
            context: Arc::new(RelayMcpContext {
                engine,
                sandbox,
                node,
                active_worktrees: Arc::default(),
                task_locks: Mutex::default(),
            }),
            tool_router: Self::tool_router(),
        }
    }

    pub async fn handle_inspect_sync_status(&self) -> RelayResult<PeerStatus> {
        if let Some(node) = &self.context.node {
            return Ok(node.peer_status());
        }
        let engine = &self.context.engine;
        Ok(PeerStatus {
            mode: engine.runtime_mode().await,
            local_node_id: String::new(),
            remote_node_id: None,
            transport_path: TransportPath::Offline,
            connected: false,
            rtt_ms: None,
            last_synced_commit: engine.head_commit_sha().await,
            pending_outbox_count: engine.pending_outbox_count().await?,
        })
    }

    pub async fn handle_list_handoffs(
        &self,
        args: ListHandoffsArgs,
    ) -> RelayResult<Vec<HandoffSummary>> {
        self.context
            .engine
            .list_handoffs(args.limit.unwrap_or(10))
            .await
    }

    pub async fn handle_check_env_diff(&self, args: CheckEnvDiffArgs) -> RelayResult<EnvDiffReport> {
        self.context
            .sandbox()?
            .check_env_diff(&args.commit_sha)
            .await
    }

    pub async fn handle_get_dag_snapshot(&self, args: GetDagSnapshotArgs) -> DagSnapshot {
        self.context
            .engine
            .dag_snapshot(args.include_stale.unwrap_or(true))
            .await
    }

    pub async fn handle_read_evidence_log(
        &self,
        args: ReadEvidenceLogArgs,
    ) -> RelayResult<EvidenceLog> {
        let (node, log_tail) = self
            .context
            .engine
            .read_evidence_log(&args.node_id, args.max_lines.unwrap_or(120))
            .await?;
        Ok(EvidenceLog { node, log_tail })
    }

    pub async fn handle_run_preflight(&self, args: RunPreflightArgs) -> RelayResult<PreflightReport> {
        let task_id = parse_task_id(&args.task_id)?;
        let context = self.context.clone();
        finish_operation(async move {
            let _guard = context.lock_task(task_id).await;
            let task = context.task(task_id).await?;
            context.run_preflight(&task).await
        })
        .await
    }

    pub async fn handle_propose_patch(
        &self,
        args: ProposeWorktreePatchArgs,
    ) -> RelayResult<PatchProposal> {
        let task_id = parse_task_id(&args.task_id)?;
        let kind = match args.patch_kind.as_str() {
            "uv_spec" => PatchKind::UvSpec,
            "code_diff" => PatchKind::CodeDiff,
            kind => return Err(RelayError::Internal(format!("unknown patch kind '{kind}'"))),
        };
        let context = self.context.clone();
        finish_operation(async move {
            let _guard = context.lock_task(task_id).await;
            let task = context.task(task_id).await?;
            context.propose_patch(&task, kind, &args).await
        })
        .await
    }

    pub async fn handle_approve_and_execute(
        &self,
        args: ApproveAndExecuteArgs,
        request: Option<&RequestContext<RoleServer>>,
    ) -> RelayResult<ApprovalExecutionResponse> {
        let task_id = parse_task_id(&args.task_id)?;
        let context = self.context.clone();
        let guard = context.lock_task(task_id).await;
        let task = context.task(task_id).await?;
        let proposal = task.active_proposal.as_ref();
        let patch_id = proposal.map(|patch| patch.patch_id.as_str());
        if let Some(requested) = args.patch_id.as_deref()
            && Some(requested) != patch_id
        {
            return Err(RelayError::ProposalNotFound(requested.into()));
        }
        match task.state {
            TaskState::Completed => {
                context.cleanup_worktree(&task).await?;
                let (_, worktree_branch) = context.sandbox()?.task_worktree(task_id)?;
                return Ok(ApprovalExecutionResponse::ExecutionStarted {
                    task_id: task_id.to_string(),
                    worktree_branch,
                });
            }
            TaskState::Rejected => {
                context.cleanup_worktree(&task).await?;
                return Ok(rejected(task_id));
            }
            TaskState::Executing => {}
            state => validate_transition(task_id, state, TaskState::Executing)?,
        }

        let decision = if task.state != TaskState::Executing
            && (task.spec.require_approval
                || task.state == TaskState::SelfHealingProposed
                || proposal.is_some())
            && !args.approved_via_ui.unwrap_or(false)
        {
            let summary = proposal.map_or_else(
                || format!("Approve running preset '{}'", task.spec.command_preset),
                |patch| patch.summary.clone(),
            );
            let diff_preview = proposal.map_or_else(String::new, |patch| patch.diff_preview.clone());
            let decision = match request {
                Some(request) => {
                    elicit_approval(request, task_id, &summary, &diff_preview).await
                }
                None => None,
            };
            let Some(decision) = decision else {
                return Ok(ApprovalExecutionResponse::ApprovalRequired {
                    task_id: task_id.to_string(),
                    patch_id: patch_id.map(str::to_owned),
                    summary,
                    diff_preview,
                });
            };
            decision
        } else {
            ApprovalDecision::Approved
        };

        // Keep the task and its worktree locked until the decision and resulting work finish.
        finish_operation(async move {
            let _guard = guard;
            context.finish_approval(task, decision).await
        })
        .await
    }
}

impl RelayMcpContext {
    fn sandbox(&self) -> RelayResult<&WorktreeSandbox> {
        self.sandbox.as_ref().ok_or_else(|| {
            RelayError::Internal("no worktree sandbox is configured for this server".into())
        })
    }

    async fn task(&self, task_id: TaskId) -> RelayResult<TaskRecord> {
        self.engine
            .get_task(task_id)
            .await?
            .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))
    }

    async fn lock_task(&self, task_id: TaskId) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.task_locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() != 0);
            if let Some(lock) = locks.get(&task_id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(task_id, Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    async fn find_worktree(&self, task: &TaskRecord) -> RelayResult<Option<WorktreeHandle>> {
        let id = task.spec.task_id;
        if let Some(handle) = self.active_worktrees.lock().await.get(&id).cloned() {
            return Ok(Some(handle));
        }
        match &self.sandbox {
            Some(sandbox) => sandbox.reopen_worktree(id, &task.spec.base_commit_sha).await,
            None => Ok(None),
        }
    }

    async fn worktree(&self, task: &TaskRecord) -> RelayResult<WorktreeHandle> {
        let sandbox = self.sandbox()?;
        let id = task.spec.task_id;
        let handle = match self.find_worktree(task).await? {
            Some(handle) => handle,
            None if task.active_proposal.is_some() => {
                return Err(RelayError::Internal("verified patch worktree is missing".into()));
            }
            None => sandbox.prepare_worktree(id, &task.spec.base_commit_sha).await?,
        };
        self.active_worktrees.lock().await.insert(id, handle.clone());
        Ok(handle)
    }

    async fn cleanup_worktree(&self, task: &TaskRecord) -> RelayResult<()> {
        if let Some(handle) = self.find_worktree(task).await? {
            handle.cleanup().await?;
            self.active_worktrees.lock().await.remove(&task.spec.task_id);
        }
        Ok(())
    }

    async fn cleanup_after_error(&self, task: &TaskRecord) {
        if let Err(error) = self.cleanup_worktree(task).await {
            tracing::error!(task_id = ?task.spec.task_id, ?error, "Worktree cleanup failed");
        }
    }

    async fn start_preflight(&self, task: &TaskRecord) -> RelayResult<()> {
        if task.state != TaskState::PreflightRunning {
            self.engine
                .transition_task(task.spec.task_id, TaskState::PreflightRunning, None, None)
                .await?;
        }
        Ok(())
    }

    async fn run_preflight(&self, task: &TaskRecord) -> RelayResult<PreflightReport> {
        let sandbox = self.sandbox()?;
        self.start_preflight(task).await?;
        let id = task.spec.task_id;
        let result = async {
            let handle = self.worktree(task).await?;
            let manifest = load_manifest(handle.worktree_path())?;
            let report = sandbox
                .run_preflight(id, handle.worktree_path(), handle.branch_name(), &manifest)
                .await?;
            let summary = if report.smoke_passed {
                "Smoke preflight passed".into()
            } else if let Some(module) = &report.detected_missing_module {
                format!("Smoke preflight failed: missing Python module {module}")
            } else {
                format!("Smoke preflight failed (exit code {:?})", report.exit_code)
            };
            self.engine.set_preflight_summary(id, summary).await?;
            Ok(report)
        }
        .await;
        if result.is_err() {
            self.cleanup_after_error(task).await;
        }
        result
    }

    async fn propose_patch(
        &self,
        task: &TaskRecord,
        kind: PatchKind,
        args: &ProposeWorktreePatchArgs,
    ) -> RelayResult<PatchProposal> {
        let sandbox = self.sandbox()?;
        self.start_preflight(task).await?;
        let result = async {
            let handle = self.worktree(task).await?;
            let manifest = load_manifest(handle.worktree_path())?;
            let proposal = sandbox
                .apply_and_verify_patch(
                    task.spec.task_id,
                    handle.worktree_path(),
                    handle.branch_name(),
                    &manifest,
                    kind,
                    &args.summary,
                    &args.content,
                )
                .await?;
            if !proposal.smoke_verified {
                return Err(RelayError::Internal(format!(
                    "patch failed smoke verification:\n{}",
                    proposal.smoke_stdout_tail,
                )));
            }
            let task = self.engine.attach_patch_proposal(proposal).await?;
            task.active_proposal.ok_or_else(|| {
                RelayError::Internal("verified patch was not recorded".into())
            })
        }
        .await;
        if result.is_err() {
            self.cleanup_after_error(task).await;
        }
        result
    }

    async fn finish_approval(
        &self,
        task: TaskRecord,
        decision: ApprovalDecision,
    ) -> RelayResult<ApprovalExecutionResponse> {
        let id = task.spec.task_id;
        let patch_id = task
            .active_proposal
            .as_ref()
            .map(|patch| patch.patch_id.as_str());
        if decision == ApprovalDecision::Rejected {
            self.engine.resolve_approval(id, patch_id, decision).await?;
            self.cleanup_worktree(&task).await?;
            return Ok(rejected(id));
        }

        let sandbox = self.sandbox()?;
        let task = if task.state == TaskState::Executing {
            task
        } else {
            self.engine.resolve_approval(id, patch_id, decision).await?
        };
        match self.execute(sandbox, &task).await {
            Ok(response) => {
                self.cleanup_worktree(&task).await?;
                Ok(response)
            }
            Err(error) => {
                self.cleanup_after_error(&task).await;
                if let Err(state_error) = self
                    .engine
                    .transition_task(id, TaskState::Failed, None, Some(error.to_string()))
                    .await
                {
                    tracing::error!(
                        task_id = ?id,
                        ?state_error,
                        "Unable to record preset failure in the current task state"
                    );
                }
                Err(error)
            }
        }
    }

    async fn execute(
        &self,
        sandbox: &WorktreeSandbox,
        task: &TaskRecord,
    ) -> RelayResult<ApprovalExecutionResponse> {
        let spec = &task.spec;
        let handle = self.worktree(task).await?;
        sandbox.validate_worktree_base(spec.task_id, &handle, &spec.base_commit_sha).await?;
        let diff = handle.diff().await?;
        if let Some(patch) = &task.active_proposal {
            if !patch.smoke_verified
                || patch.worktree_branch != handle.branch_name()
                || patch.diff_preview != diff
            {
                return Err(RelayError::Internal(
                    "worktree differs from the verified proposal".into(),
                ));
            }
        } else if !diff.is_empty() {
            return Err(RelayError::Internal(
                "task worktree has changes without an approved patch".into(),
            ));
        }

        let manifest = load_manifest(handle.worktree_path())?;
        let output = sandbox
            .run_preset(handle.worktree_path(), &manifest, &spec.command_preset, &spec.params)
            .await?;
        if !output.success {
            let detail = output.raw_log.lines().rev().find(|line| !line.trim().is_empty());
            return Err(RelayError::Internal(format!(
                "preset '{}' failed (exit code {:?}){}",
                spec.command_preset,
                output.exit_code,
                detail.map_or_else(String::new, |line| format!(": {line}")),
            )));
        }

        let sha = &spec.base_commit_sha;
        let (env_fingerprint, changed_files) = tokio::try_join!(
            sandbox.env_fingerprint_at_commit(sha),
            sandbox.changed_files_in_commit(sha),
        )?;
        let now = Utc::now();
        let device_origin = self
            .node
            .as_ref()
            .map_or_else(|| "local".into(), |node| node.id().to_string());
        let artifact = GraphNode {
            id: format!("artifact:{}", Uuid::now_v7()),
            label: sha.clone(),
            payload: NodeKind::Artifact {
                git_commit_sha: sha.clone(),
                env_fingerprint,
                working_tree_dirty: false,
                changed_files,
            },
            freshness: Freshness::Fresh,
            device_origin: device_origin.clone(),
            created_at: now,
            updated_at: now,
        };
        let evidence = GraphNode {
            id: format!("evidence:{}", spec.task_id),
            label: format!("{} result", spec.command_preset),
            payload: NodeKind::Evidence {
                task_id: spec.task_id.to_string(),
                preset: spec.command_preset.clone(),
                metrics_json: output.metrics_json.unwrap_or(JsonValue::Null),
                log_blob_hash: output.log_blob_hash,
            },
            freshness: Freshness::Fresh,
            device_origin,
            created_at: now,
            updated_at: now,
        };
        self.engine
            .record_worktree_evidence(spec.task_id, &artifact, evidence, &output.raw_log)
            .await?;
        Ok(ApprovalExecutionResponse::ExecutionStarted {
            task_id: spec.task_id.to_string(),
            worktree_branch: handle.branch_name().into(),
        })
    }
}

async fn elicit_approval(
    request: &RequestContext<RoleServer>,
    task_id: TaskId,
    summary: &str,
    diff_preview: &str,
) -> Option<ApprovalDecision> {
    let message = format!("Task {task_id}\n{summary}\n{diff_preview}");
    match request
        .peer
        .elicit_with_timeout::<ApprovalForm>(message, Some(Duration::from_secs(60)))
        .await
    {
        Ok(Some(form)) if form.approve => Some(ApprovalDecision::Approved),
        Ok(_) | Err(ElicitationError::UserDeclined | ElicitationError::UserCancelled) => {
            Some(ApprovalDecision::Rejected)
        }
        Err(_) => None,
    }
}

fn rejected(task_id: TaskId) -> ApprovalExecutionResponse {
    ApprovalExecutionResponse::Rejected {
        task_id: task_id.to_string(),
        reason: "User declined or cancelled approval".into(),
    }
}

fn parse_task_id(task_id: &str) -> RelayResult<TaskId> {
    Uuid::parse_str(task_id)
        .map_err(|error| RelayError::Internal(format!("invalid task ID '{task_id}': {error}")))
}

async fn finish_operation<T: Send + 'static>(
    operation: impl Future<Output = RelayResult<T>> + Send + 'static,
) -> RelayResult<T> {
    // MCP requests can be cancelled while a worktree or task state is being updated.
    tokio::spawn(operation)
        .await
        .map_err(|error| RelayError::Internal(error.to_string()))?
}

fn tool_result<T: Serialize>(result: RelayResult<T>) -> Result<CallToolResult, McpError> {
    let text = match result {
        Ok(value) => serde_json::to_string(&value)
            .map_err(|error| McpError::internal_error(error.to_string(), None))?,
        Err(error @ RelayError::StaleEvidenceBlocked { .. }) => {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "STALE evidence must not be cited. {error}"
            ))]));
        }
        Err(error) => return Ok(CallToolResult::error(vec![ContentBlock::text(error.to_string())])),
    };
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

#[tool_router]
impl RelayMcpServer {
    #[tool(description = "Inspect P2P sync status, active transport path, and pending outbox count.")]
    async fn inspect_sync_status(&self) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_inspect_sync_status().await)
    }

    #[tool(description = "List recent cross-device handoffs with base commit and active task count.")]
    async fn list_handoffs(
        &self,
        Parameters(args): Parameters<ListHandoffsArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_list_handoffs(args).await)
    }

    #[tool(description = "Compare target commit uv.lock and pyproject environment fingerprints against the Host.")]
    async fn check_env_diff(
        &self,
        Parameters(args): Parameters<CheckEnvDiffArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_check_env_diff(args).await)
    }

    #[tool(description = "Fetch the Decision-Evidence DAG snapshot with explicit STALE flags and count.")]
    async fn get_dag_snapshot(
        &self,
        Parameters(args): Parameters<GetDagSnapshotArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(Ok(self.handle_get_dag_snapshot(args).await))
    }

    #[tool(description = "Read evidence metrics and the log tail. STALE evidence is blocked.")]
    async fn read_evidence_log(
        &self,
        Parameters(args): Parameters<ReadEvidenceLogArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_read_evidence_log(args).await)
    }

    #[tool(description = "Run smoke preflight in the task's isolated Git worktree.")]
    async fn run_preflight_in_worktree(
        &self,
        Parameters(args): Parameters<RunPreflightArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_run_preflight(args).await)
    }

    #[tool(description = "Apply a uv_spec or code_diff patch in the task worktree, verify smoke, and persist the proposal.")]
    async fn propose_worktree_patch(
        &self,
        Parameters(args): Parameters<ProposeWorktreePatchArgs>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_propose_patch(args).await)
    }

    #[tool(description = "Request human approval when needed and run the task's whitelisted experiment preset.")]
    async fn approve_and_execute_task(
        &self,
        Parameters(args): Parameters<ApproveAndExecuteArgs>,
        request: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        tool_result(self.handle_approve_and_execute(args, Some(&request)).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RelayMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[cfg(test)]
mod tests;
