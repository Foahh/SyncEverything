use super::DesktopRuntime;
use crate::{AgentChatStreamEvent, emit_state};
use chrono::Utc;
use relay_core::agent::{
    AgentApprovalCard, AgentChatMessage, AgentChatRole, AgentToolTrace, COPILOT_SYSTEM_PROMPT,
    LlmClient, ToolTraceStatus, copilot_tool_definitions,
};
use relay_core::contracts::{
    ApprovalExecutionResponse, ApproveAndExecuteArgs, CheckEnvDiffArgs, EdgeKind, Freshness,
    GetDagSnapshotArgs, GraphEdge, GraphNode, ListHandoffsArgs, NodeKind, ProposeWorktreePatchArgs,
    ReadEvidenceLogArgs, RunPreflightArgs, RuntimeMode, TaskRecord, TaskSpec, TaskState,
};
use relay_core::error::{RelayError, RelayResult};
use relay_core::sandbox::{WorktreeSandbox, load_manifest, resolve_preset};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri_specta::Event;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

const MAX_COPILOT_TURNS: usize = 8;
const MAX_IDENTICAL_CALLS: u32 = 3;
const HISTORY_WINDOW: u32 = 16;

pub(super) struct ChatTurn {
    cancel: Arc<AtomicBool>,
    stop_stream: Option<oneshot::Sender<()>>,
    worker: JoinHandle<()>,
}

// The lifecycle lock serializes send/clear/cancel. The worker never takes that lock.
async fn finish_previous_turn(active: &mut Option<ChatTurn>) -> bool {
    let Some(turn) = active.as_mut() else { return false };
    let was_running = !turn.worker.is_finished();
    turn.cancel.store(true, Ordering::Relaxed);
    if let Some(stop) = turn.stop_stream.take() {
        let _ = stop.send(());
    }
    if let Err(error) = (&mut turn.worker).await {
        tracing::warn!(%error, "Copilot turn stopped before it finished");
    }
    *active = None;
    was_running
}

struct ToolExecOutcome {
    output_json: String,
    ok: bool,
    mutated_graph: bool,
    approval_card: Option<AgentApprovalCard>,
}

#[derive(Deserialize)]
struct SubmitTaskToolArgs {
    preset: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    #[serde(default)]
    user_note: Option<String>,
    #[serde(default)]
    require_approval: Option<bool>,
}

#[derive(Deserialize)]
struct RecordDecisionToolArgs {
    label: String,
    rationale: String,
    #[serde(default)]
    constraints: Vec<String>,
}

#[derive(Deserialize)]
struct ReadRepoFileToolArgs {
    path: String,
}

impl DesktopRuntime {
    pub(crate) async fn list_chat_messages(
        &self,
        limit: Option<u32>,
    ) -> RelayResult<Vec<AgentChatMessage>> {
        self.engine
            .list_chat_messages(limit.unwrap_or(100).clamp(1, 500))
            .await
    }

    pub(crate) async fn cancel_chat_turn(&self) -> bool {
        let mut active = self.active_chat_turn.lock().await;
        finish_previous_turn(&mut active).await
    }

    pub(crate) async fn clear_chat_messages(&self) -> RelayResult<()> {
        let mut active = self.active_chat_turn.lock().await;
        finish_previous_turn(&mut active).await;
        self.engine.clear_chat_messages().await?;
        emit_state(&self.app, "chat_updated", "Chat history cleared");
        Ok(())
    }

    pub(crate) async fn send_chat_message(
        self: &Arc<Self>,
        prompt: String,
    ) -> RelayResult<AgentChatMessage> {
        let trimmed = prompt.trim().to_owned();
        if trimmed.is_empty() {
            return Err(RelayError::Internal("Message cannot be empty".into()));
        }

        let mut active = self.active_chat_turn.lock().await;
        finish_previous_turn(&mut active).await;

        let now = Utc::now();
        let repo_id = self.engine.repo_id().to_owned();
        let user_msg = AgentChatMessage {
            id: Uuid::now_v7().to_string(),
            repo_id: repo_id.clone(),
            role: AgentChatRole::User,
            content: trimmed,
            traces: Vec::new(),
            approval: None,
            error: None,
            created_at: now,
        };
        let assistant_msg = AgentChatMessage {
            id: Uuid::now_v7().to_string(),
            repo_id,
            role: AgentChatRole::Assistant,
            content: String::new(),
            traces: Vec::new(),
            approval: None,
            error: None,
            created_at: now + chrono::Duration::milliseconds(1),
        };

        self.engine.upsert_chat_message(&user_msg).await?;
        self.engine.upsert_chat_message(&assistant_msg).await?;

        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (stop_stream, stopped) = oneshot::channel();
        let runtime = self.clone();
        let turn_id = Uuid::now_v7().to_string();
        let initial = assistant_msg.clone();
        let worker = tokio::spawn(async move {
            runtime.run_copilot_turn(turn_id, assistant_msg, worker_cancel, stopped).await;
        });
        *active = Some(ChatTurn { cancel, stop_stream: Some(stop_stream), worker });

        Ok(initial)
    }

    async fn run_copilot_turn(
        self: Arc<Self>,
        turn_id: String,
        mut assistant_msg: AgentChatMessage,
        cancel: Arc<AtomicBool>,
        mut stopped: oneshot::Receiver<()>,
    ) {
        let message_id = assistant_msg.id.clone();
        self.emit_chat_event(AgentChatStreamEvent {
            turn_id: turn_id.clone(),
            message_id: message_id.clone(),
            kind: "turn_started".into(),
            delta: None,
            trace: None,
            approval: None,
            error: None,
        });

        let llm_config = self.llm_config.read().await.clone();
        let llm = match LlmClient::new(llm_config, Duration::from_secs(90)) {
            Ok(client) => client,
            Err(error) => {
                self.finish_turn_with_error(
                    &turn_id,
                    &mut assistant_msg,
                    error.to_string(),
                )
                .await;
                return;
            }
        };

        let mut messages = match self.build_copilot_messages(&message_id).await {
            Ok(messages) => messages,
            Err(error) => {
                self.finish_turn_with_error(&turn_id, &mut assistant_msg, error.to_string())
                    .await;
                return;
            }
        };
        let tools = copilot_tool_definitions();
        let mut seen_calls: HashMap<String, u32> = HashMap::new();

        'turns: for _turn_idx in 0..MAX_COPILOT_TURNS {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            let app_handle = self.app.clone();
            let turn_id_cb = turn_id.clone();
            let message_id_cb = message_id.clone();
            let request = llm
                .stream_chat_with_tools(&messages, &tools, &cancel, |delta| {
                    let event = AgentChatStreamEvent {
                        turn_id: turn_id_cb.clone(),
                        message_id: message_id_cb.clone(),
                        kind: "token_delta".into(),
                        delta: Some(delta.to_owned()),
                        trace: None,
                        approval: None,
                        error: None,
                    };
                    let _ = event.emit(&app_handle);
                });
            let streamed = tokio::select! {
                biased;
                _ = &mut stopped => break,
                response = request => response,
            };

            let turn = match streamed {
                Ok(turn) => turn,
                Err(RelayError::Llm(msg)) if msg == "cancelled" => {
                    break;
                }
                Err(error) => {
                    self.finish_turn_with_error(
                        &turn_id,
                        &mut assistant_msg,
                        error.to_string(),
                    )
                    .await;
                    return;
                }
            };

            if !turn.content.is_empty() {
                if !assistant_msg.content.is_empty() && !assistant_msg.content.ends_with('\n') {
                    assistant_msg.content.push_str("\n\n");
                }
                assistant_msg.content.push_str(&turn.content);
            }

            if turn.tool_calls.is_empty() {
                break;
            }

            let tool_calls_json: Vec<Value> = turn
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments,
                        }
                    })
                })
                .collect();
            messages.push(json!({
                "role": "assistant",
                "content": if turn.content.is_empty() { Value::Null } else { Value::String(turn.content.clone()) },
                "tool_calls": tool_calls_json,
            }));

            let mut stop_for_approval = false;
            for call in turn.tool_calls {
                if cancel.load(Ordering::Relaxed) {
                    break 'turns;
                }

                let sig = format!("{}::{}", call.name, call.arguments.trim());
                let count = seen_calls.entry(sig).or_insert(0);
                *count += 1;
                if *count >= MAX_IDENTICAL_CALLS {
                    assistant_msg.error = Some(format!(
                        "Stopped repeated tool call loop ({} called {} times with identical arguments).",
                        call.name, count
                    ));
                    break 'turns;
                }

                let trace_id = format!("{}:{}", call.id, assistant_msg.traces.len());
                let mut trace = AgentToolTrace {
                    id: trace_id,
                    tool_name: call.name.clone(),
                    args_preview: truncate_preview(&call.arguments, 240),
                    status: ToolTraceStatus::Running,
                    output_preview: None,
                    duration_ms: None,
                };
                assistant_msg.traces.push(trace.clone());
                self.emit_chat_event(AgentChatStreamEvent {
                    turn_id: turn_id.clone(),
                    message_id: message_id.clone(),
                    kind: "tool_started".into(),
                    delta: None,
                    trace: Some(trace.clone()),
                    approval: None,
                    error: None,
                });

                let started = Instant::now();
                let outcome = self
                    .execute_copilot_tool(&call.name, &call.arguments, &cancel)
                    .await;
                let elapsed_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);

                trace.status = if outcome.ok {
                    ToolTraceStatus::Done
                } else {
                    ToolTraceStatus::Error
                };
                trace.output_preview = Some(truncate_preview(&outcome.output_json, 2400));
                trace.duration_ms = Some(elapsed_ms);

                if let Some(slot) = assistant_msg.traces.last_mut() {
                    *slot = trace.clone();
                }

                self.emit_chat_event(AgentChatStreamEvent {
                    turn_id: turn_id.clone(),
                    message_id: message_id.clone(),
                    kind: "tool_finished".into(),
                    delta: None,
                    trace: Some(trace),
                    approval: None,
                    error: None,
                });

                if outcome.mutated_graph {
                    emit_state(
                        &self.app,
                        "dag_changed",
                        format!("Copilot ran {}", call.name),
                    );
                }

                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": truncate_preview(&outcome.output_json, 8000),
                }));

                if let Some(card) = outcome.approval_card {
                    assistant_msg.approval = Some(card.clone());
                    if let Err(error) = self.engine.upsert_chat_message(&assistant_msg).await {
                        self.finish_turn_with_error(
                            &turn_id, &mut assistant_msg, error.to_string(),
                        ).await;
                        return;
                    }
                    self.emit_chat_event(AgentChatStreamEvent {
                        turn_id: turn_id.clone(),
                        message_id: message_id.clone(),
                        kind: "approval_needed".into(),
                        delta: None,
                        trace: None,
                        approval: Some(card),
                        error: None,
                    });
                    stop_for_approval = true;
                }
            }

            let _ = self.engine.upsert_chat_message(&assistant_msg).await;

            if stop_for_approval {
                if assistant_msg.content.trim().is_empty() {
                    let note = "A verified fix or run requires your confirmation before executing. Review the preview below and click **Approve** or **Dismiss**.";
                    assistant_msg.content = note.to_owned();
                    self.emit_chat_event(AgentChatStreamEvent {
                        turn_id: turn_id.clone(),
                        message_id: message_id.clone(),
                        kind: "token_delta".into(),
                        delta: Some(note.to_owned()),
                        trace: None,
                        approval: None,
                        error: None,
                    });
                }
                break 'turns;
            }
        }

        if let Err(error) = self.engine.upsert_chat_message(&assistant_msg).await {
            tracing::warn!(%error, "Unable to persist final Copilot message");
        }

        let kind = if assistant_msg.error.is_some() {
            "turn_error"
        } else {
            "turn_completed"
        };
        self.emit_chat_event(AgentChatStreamEvent {
            turn_id,
            message_id,
            kind: kind.into(),
            delta: None,
            trace: None,
            approval: assistant_msg.approval.clone(),
            error: assistant_msg.error.clone(),
        });
        emit_state(&self.app, "chat_updated", "Copilot response saved");
    }

    async fn finish_turn_with_error(
        &self,
        turn_id: &str,
        assistant_msg: &mut AgentChatMessage,
        error: String,
    ) {
        assistant_msg.error = Some(error.clone());
        let _ = self.engine.upsert_chat_message(assistant_msg).await;
        self.emit_chat_event(AgentChatStreamEvent {
            turn_id: turn_id.to_owned(),
            message_id: assistant_msg.id.clone(),
            kind: "turn_error".into(),
            delta: None,
            trace: None,
            approval: None,
            error: Some(error),
        });
        emit_state(&self.app, "chat_updated", "Copilot turn ended with error");
    }

    fn emit_chat_event(&self, event: AgentChatStreamEvent) {
        if let Err(error) = event.emit(&self.app) {
            tracing::warn!(%error, kind = %event.kind, "Unable to emit Copilot stream event");
        }
    }

    async fn build_copilot_messages(&self, exclude_message_id: &str) -> RelayResult<Vec<Value>> {
        let snapshot = self.engine.dag_snapshot(true).await?;
        let status = self.status().await;
        let tasks = self.engine.list_tasks().await?;
        let active_tasks: Vec<Value> = tasks
            .iter()
            .take(6)
            .map(|t| {
                json!({
                    "task_id": t.spec.task_id,
                    "preset": t.spec.command_preset,
                    "state": t.state,
                    "patch_id": t.active_proposal.as_ref().map(|p| p.patch_id.clone()),
                })
            })
            .collect();
        let workspace = self.workspace_info();
        let presets: Vec<Value> = workspace
            .presets
            .iter()
            .map(|p| {
                json!({
                    "name": p.name,
                    "description": p.description,
                    "allowed_params": p.allowed_params,
                })
            })
            .collect();

        let workspace_ctx = format!(
            "{COPILOT_SYSTEM_PROMPT}\n\n[ACTIVE WORKSPACE CONTEXT]\nrepo_id: {}\nname: {}\nruntime_mode: {:?}\nhead_commit_sha: {}\nstale_evidence_count: {}\nwhitelisted_presets: {}\nrecent_tasks: {}",
            workspace.repo_id,
            workspace.name,
            status.mode,
            snapshot.head_commit_sha.as_deref().unwrap_or("none"),
            snapshot.stale_count,
            serde_json::to_string(&presets).unwrap_or_default(),
            serde_json::to_string(&active_tasks).unwrap_or_default(),
        );

        let mut messages = vec![json!({
            "role": "system",
            "content": workspace_ctx,
        })];

        let history = self
            .engine
            .list_chat_messages(HISTORY_WINDOW + 2)
            .await?;
        for msg in history {
            if msg.id == exclude_message_id || msg.content.trim().is_empty() {
                continue;
            }
            let role = match msg.role {
                AgentChatRole::User => "user",
                AgentChatRole::Assistant => "assistant",
            };
            messages.push(json!({
                "role": role,
                "content": msg.content,
            }));
        }
        Ok(messages)
    }

    async fn execute_copilot_tool(
        self: &Arc<Self>,
        name: &str,
        raw_args: &str,
        cancel: &AtomicBool,
    ) -> ToolExecOutcome {
        let parsed_args: Value = serde_json::from_str(raw_args).unwrap_or_else(|_| json!({}));

        match name {
            "inspect_sync_status" => ok_json(&self.status().await),
            "list_handoffs" => {
                let args: ListHandoffsArgs =
                    serde_json::from_value(parsed_args).unwrap_or(ListHandoffsArgs { limit: Some(10) });
                result_json(self.mcp.handle_list_handoffs(args).await)
            }
            "check_env_diff" => {
                let commit_sha = parsed_args
                    .get("commit_sha")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("HEAD")
                    .to_owned();
                result_json(
                    self.mcp
                        .handle_check_env_diff(CheckEnvDiffArgs { commit_sha })
                        .await,
                )
            }
            "get_dag_snapshot" => {
                let include_stale = parsed_args
                    .get("include_stale")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                result_json(
                    self
                        .mcp
                        .handle_get_dag_snapshot(GetDagSnapshotArgs {
                            include_stale: Some(include_stale),
                        })
                        .await,
                )
            }
            "read_evidence_log" => match serde_json::from_value::<ReadEvidenceLogArgs>(parsed_args) {
                Ok(args) => result_json(self.mcp.handle_read_evidence_log(args).await),
                Err(error) => err_outcome(format!("Invalid arguments: {error}")),
            },
            "run_preflight_in_worktree" => {
                match serde_json::from_value::<RunPreflightArgs>(parsed_args) {
                    Ok(args) => result_json(self.mcp.handle_run_preflight(args).await),
                    Err(error) => err_outcome(format!("Invalid arguments: {error}")),
                }
            }
            "propose_worktree_patch" => {
                match serde_json::from_value::<ProposeWorktreePatchArgs>(parsed_args) {
                    Ok(args) => {
                        let mut outcome = result_json(self.mcp.handle_propose_patch(args).await);
                        outcome.mutated_graph = outcome.ok;
                        outcome
                    }
                    Err(error) => err_outcome(format!("Invalid arguments: {error}")),
                }
            }
            "approve_and_execute_task" => {
                let task_id = match parsed_args.get("task_id").and_then(Value::as_str) {
                    Some(id) if !id.trim().is_empty() => id.to_owned(),
                    _ => return err_outcome("Missing required 'task_id' argument".into()),
                };
                let patch_id = parsed_args
                    .get("patch_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                match self
                    .mcp
                    .handle_approve_and_execute(
                        ApproveAndExecuteArgs {
                            task_id,
                            patch_id,
                            approved_via_ui: Some(false),
                        },
                        None,
                    )
                    .await
                {
                    Ok(resp @ ApprovalExecutionResponse::ApprovalRequired { .. }) => {
                        let card = match &resp {
                            ApprovalExecutionResponse::ApprovalRequired {
                                task_id,
                                patch_id,
                                summary,
                                diff_preview,
                            } => Some(AgentApprovalCard {
                                task_id: task_id.clone(),
                                patch_id: patch_id.clone(),
                                summary: summary.clone(),
                                diff_preview: diff_preview.clone(),
                                resolved: None,
                            }),
                            _ => None,
                        };
                        ToolExecOutcome {
                            output_json: serde_json::to_string_pretty(&resp).unwrap_or_default(),
                            ok: true,
                            mutated_graph: false,
                            approval_card: card,
                        }
                    }
                    Ok(resp) => {
                        let mut outcome = ok_json(&resp);
                        outcome.mutated_graph = true;
                        outcome
                    }
                    Err(error) => err_outcome(error.to_string()),
                }
            }
            "submit_task" => self.tool_submit_task(parsed_args, cancel).await,
            "record_decision" => self.tool_record_decision(parsed_args).await,
            "read_repo_file" => self.tool_read_repo_file(parsed_args).await,
            "git_diff" => self.tool_git_diff().await,
            unknown => err_outcome(format!("Unknown Copilot tool '{unknown}'")),
        }
    }

    async fn tool_submit_task(
        self: &Arc<Self>,
        parsed_args: Value,
        cancel: &AtomicBool,
    ) -> ToolExecOutcome {
        let args: SubmitTaskToolArgs = match serde_json::from_value(parsed_args) {
            Ok(args) => args,
            Err(error) => return err_outcome(format!("Invalid submit_task arguments: {error}")),
        };

        // Strictly validate against the active repo's relay.toml whitelist.
        if let Some(sandbox) = &self.sandbox {
            let manifest = match load_manifest(sandbox.repo_root()) {
                Ok(m) => m,
                Err(error) => return err_outcome(format!("Cannot load relay.toml: {error}")),
            };
            let preset = match resolve_preset(&manifest, &args.preset) {
                Ok(p) => p,
                Err(error) => return err_outcome(error.to_string()),
            };
            for key in args.params.keys() {
                if !preset.allowed_params.iter().any(|allowed| allowed == key) {
                    return err_outcome(format!(
                        "Parameter '{key}' is not in allowed_params for preset '{}'",
                        args.preset
                    ));
                }
            }
        } else if !self
            .workspace
            .presets
            .iter()
            .any(|p| p.name == args.preset)
        {
            return err_outcome(format!("Unknown preset '{}'", args.preset));
        }

        let base_commit_sha = if let Some(sandbox) = &self.sandbox {
            sandbox
                .head_commit_sha()
                .await
                .unwrap_or_else(|_| "HEAD".to_owned())
        } else {
            match self.engine.dag_snapshot(true).await {
                Ok(snapshot) => snapshot.head_commit_sha.unwrap_or_else(|| "HEAD".to_owned()),
                Err(error) => return err_outcome(error.to_string()),
            }
        };

        let spec = TaskSpec {
            task_id: Uuid::now_v7(),
            repo_id: self.engine.repo_id().to_owned(),
            base_commit_sha,
            command_preset: args.preset,
            params: args.params,
            require_approval: args.require_approval.unwrap_or(true),
            requested_by_device: "relay-copilot".into(),
            user_note: args.user_note,
        };
        let submitted = match self.submit(spec).await {
            Ok(record) => record,
            Err(error) => return err_outcome(error.to_string()),
        };

        let mode = self.engine.runtime_mode().await;
        let mut latest = submitted;
        if mode != RuntimeMode::Client && self.sandbox.is_some() {
            // Give the local Host preflight pipeline a brief window to settle so the Copilot
            // can immediately report completion or surface an inline Approval card.
            for _ in 0..32 {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if awaiting_clean_run_approval(&latest) || !matches!(
                    latest.state,
                    TaskState::Queued | TaskState::PreflightRunning
                ) {
                    break;
                }
                sleep(Duration::from_millis(250)).await;
                if let Ok(Some(updated)) = self.engine.get_task(latest.spec.task_id).await {
                    latest = updated;
                }
            }
        }

        let approval_card = if latest.state == TaskState::SelfHealingProposed {
            latest.active_proposal.as_ref().map(|proposal| AgentApprovalCard {
                task_id: latest.spec.task_id.to_string(),
                patch_id: Some(proposal.patch_id.clone()),
                summary: proposal.summary.clone(),
                diff_preview: proposal.diff_preview.clone(),
                resolved: None,
            })
        } else if awaiting_clean_run_approval(&latest) {
            Some(AgentApprovalCard {
                task_id: latest.spec.task_id.to_string(),
                patch_id: None,
                summary: latest.spec.command_preset.clone(),
                diff_preview: String::new(),
                resolved: None,
            })
        } else {
            None
        };

        ToolExecOutcome {
            output_json: serde_json::to_string_pretty(&latest).unwrap_or_default(),
            ok: true,
            mutated_graph: true,
            approval_card,
        }
    }

    async fn tool_record_decision(&self, parsed_args: Value) -> ToolExecOutcome {
        let args: RecordDecisionToolArgs = match serde_json::from_value(parsed_args) {
            Ok(args) => args,
            Err(error) => return err_outcome(format!("Invalid record_decision arguments: {error}")),
        };
        let label = args.label.trim().to_owned();
        let rationale = args.rationale.trim().to_owned();
        if label.is_empty() || rationale.is_empty() {
            return err_outcome("Both 'label' and 'rationale' are required".into());
        }

        let latest_artifact_id = match self.engine.head_artifact_id().await {
            Ok(id) => id,
            Err(error) => return err_outcome(error.to_string()),
        };

        let now = Utc::now();
        let node_id = format!("decision:{}", Uuid::now_v7());
        let node = GraphNode {
            id: node_id.clone(),
            label,
            payload: NodeKind::Decision {
                rationale,
                constraints: args.constraints,
            },
            freshness: Freshness::Fresh,
            device_origin: "relay-copilot".into(),
            created_at: now,
            updated_at: now,
        };
        let edges = latest_artifact_id
            .map(|artifact_id| {
                vec![GraphEdge {
                    id: format!("informs:{node_id}->{artifact_id}"),
                    from: node_id.clone(),
                    to: artifact_id,
                    kind: EdgeKind::Informs,
                }]
            })
            .unwrap_or_default();

        match self.engine.upsert_node_and_edges(node.clone(), edges).await {
            Ok(_) => ToolExecOutcome {
                output_json: serde_json::to_string_pretty(&json!({
                    "recorded": true,
                    "node_id": node.id,
                    "label": node.label,
                }))
                .unwrap_or_default(),
                ok: true,
                mutated_graph: true,
                approval_card: None,
            },
            Err(error) => err_outcome(error.to_string()),
        }
    }

    async fn tool_read_repo_file(&self, parsed_args: Value) -> ToolExecOutcome {
        let args: ReadRepoFileToolArgs = match serde_json::from_value(parsed_args) {
            Ok(args) => args,
            Err(error) => return err_outcome(format!("Invalid read_repo_file arguments: {error}")),
        };
        let Some(sandbox) = self.repo_reader() else {
            return err_outcome("No active local repository is configured".into());
        };
        match sandbox.read_repo_file(&args.path, 12_000).await {
            Ok(content) => ToolExecOutcome {
                output_json: content,
                ok: true,
                mutated_graph: false,
                approval_card: None,
            },
            Err(error) => err_outcome(error.to_string()),
        }
    }

    async fn tool_git_diff(&self) -> ToolExecOutcome {
        let Some(sandbox) = self.repo_reader() else {
            return err_outcome("No active local repository is configured".into());
        };
        match sandbox.git_status_and_diff(12_000).await {
            Ok(diff) => ToolExecOutcome {
                output_json: diff,
                ok: true,
                mutated_graph: false,
                approval_card: None,
            },
            Err(error) => err_outcome(error.to_string()),
        }
    }

    fn repo_reader(&self) -> Option<WorktreeSandbox> {
        self.sandbox.clone().or_else(|| {
            self.engine
                .repo_root()
                .map(|root| WorktreeSandbox::new(root, self.repo_data_dir.join("worktrees")))
        })
    }
}

fn awaiting_clean_run_approval(task: &TaskRecord) -> bool {
    task.state == TaskState::PreflightRunning
        && task.spec.require_approval
        && task.active_proposal.is_none()
        && task.preflight_summary.as_deref() == Some("Smoke preflight passed")
}

fn ok_json<T: serde::Serialize>(value: &T) -> ToolExecOutcome {
    ToolExecOutcome {
        output_json: serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into()),
        ok: true,
        mutated_graph: false,
        approval_card: None,
    }
}

fn result_json<T: serde::Serialize>(result: RelayResult<T>) -> ToolExecOutcome {
    match result {
        Ok(value) => ok_json(&value),
        Err(error) => err_outcome(error.to_string()),
    }
}

fn err_outcome(message: String) -> ToolExecOutcome {
    ToolExecOutcome {
        output_json: json!({ "error": message }).to_string(),
        ok: false,
        mutated_graph: false,
        approval_card: None,
    }
}

fn truncate_preview(raw: &str, max_chars: usize) -> String {
    let mut chars = raw.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}
