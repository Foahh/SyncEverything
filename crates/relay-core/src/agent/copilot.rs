use super::LlmClient;
use crate::error::{RelayError, RelayResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use specta::Type;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum ToolTraceStatus {
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct AgentToolTrace {
    pub id: String,
    pub tool_name: String,
    pub args_preview: String,
    pub status: ToolTraceStatus,
    pub output_preview: Option<String>,
    pub duration_ms: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct AgentApprovalCard {
    pub task_id: String,
    pub patch_id: Option<String>,
    pub summary: String,
    pub diff_preview: String,
    /// `None` while awaiting human confirmation; `Some("approved")` or `Some("rejected")` once resolved.
    pub resolved: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct AgentChatMessage {
    pub id: String,
    pub repo_id: String,
    pub role: AgentChatRole,
    pub content: String,
    #[serde(default)]
    pub traces: Vec<AgentToolTrace>,
    #[serde(default)]
    pub approval: Option<AgentApprovalCard>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamedChatTurn {
    pub content: String,
    pub tool_calls: Vec<LlmToolCall>,
}

pub const COPILOT_SYSTEM_PROMPT: &str = concat!(
    "You are the built-in Relay Agent inside the Relay cross-device workbench. ",
    "Respond in the same language the user writes in (Chinese or English). ",
    "Hard rules:\n",
    "1. Always call `get_dag_snapshot` before stating experiment metrics. NEVER cite or treat `stale` (Outdated) evidence metrics as valid for the current commit; explain why they are outdated and offer to re-run.\n",
    "2. Never run arbitrary shell commands. Only submit tasks matching presets declared in `<repo>/relay.toml` via `submit_task`.\n",
    "3. Use `read_repo_file` and `git_diff` to inspect repository code before proposing a `code_diff` patch.\n",
    "4. For a task_id on Host: follow `run_preflight_in_worktree` -> (if failed) `propose_worktree_patch` -> `approve_and_execute_task`.\n",
    "5. If `approve_and_execute_task` returns `approval_required`, STOP immediately and ask the user to confirm using the inline approval card — never loop.\n",
    "6. Keep responses concise, structured, and grounded strictly in tool outputs."
);

pub fn copilot_tool_definitions() -> Vec<Value> {
    vec![
        tool_def(
            "inspect_sync_status",
            "Inspect P2P sync status, runtime mode (client/host/local_dual_sandbox), active transport path, RTT, and pending outbox count.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool_def(
            "list_handoffs",
            "List recent cross-device handoffs with base commit SHA, summary note, active task count, and stale evidence count.",
            json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                }
            }),
        ),
        tool_def(
            "check_env_diff",
            "Compare a target Git commit's pyproject.toml and uv.lock environment fingerprint against the Host environment and list missing Python packages.",
            json!({
                "type": "object",
                "properties": {
                    "commit_sha": { "type": "string", "description": "Git commit SHA or 'HEAD'" }
                },
                "required": ["commit_sha"]
            }),
        ),
        tool_def(
            "get_dag_snapshot",
            "Fetch the Decision-Evidence DAG snapshot with head_commit_sha, nodes, edges, stale_count, and explicit freshness flags. NEVER cite metrics from stale nodes.",
            json!({
                "type": "object",
                "properties": {
                    "include_stale": { "type": "boolean", "default": true }
                }
            }),
        ),
        tool_def(
            "read_evidence_log",
            "Read verified metrics and log tail for a specific FRESH evidence node_id. Deterministically blocks STALE evidence nodes.",
            json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "max_lines": { "type": "integer", "minimum": 1, "maximum": 500 }
                },
                "required": ["node_id"]
            }),
        ),
        tool_def(
            "run_preflight_in_worktree",
            "Run the repository smoke preflight inside an isolated Git worktree for task_id without modifying the main working tree.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"]
            }),
        ),
        tool_def(
            "propose_worktree_patch",
            "After a failed preflight, apply a candidate fix inside the task's isolated Git worktree, re-run smoke check, and persist a PatchProposal.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "patch_kind": { "type": "string", "enum": ["uv_spec", "code_diff"] },
                    "summary": { "type": "string" },
                    "content": { "type": "string", "description": "PEP-508 package spec for uv_spec or unified git diff for code_diff" }
                },
                "required": ["task_id", "patch_kind", "summary", "content"]
            }),
        ),
        tool_def(
            "approve_and_execute_task",
            "Request human approval when required and run the task's whitelisted relay.toml preset. If approval_required is returned, stop and wait for user approval.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "patch_id": { "type": "string" }
                },
                "required": ["task_id"]
            }),
        ),
        tool_def(
            "submit_task",
            "Submit a whitelisted experiment preset from relay.toml bound to current HEAD (routes over iroh in Client mode or executes on Host/Local Test).",
            json!({
                "type": "object",
                "properties": {
                    "preset": { "type": "string", "description": "Preset name from relay.toml (e.g. 'train_grid' or 'smoke')" },
                    "params": {
                        "type": "object",
                        "additionalProperties": { "type": "string" },
                        "description": "Optional allowed_params key-value map"
                    },
                    "user_note": { "type": "string" },
                    "require_approval": { "type": "boolean", "default": true }
                },
                "required": ["preset"]
            }),
        ),
        tool_def(
            "record_decision",
            "Record a Decision note node in the DAG explaining why a change or experiment is being made, linked to the latest Code Artifact.",
            json!({
                "type": "object",
                "properties": {
                    "label": { "type": "string", "description": "Short decision title" },
                    "rationale": { "type": "string", "description": "Why this decision was made" },
                    "constraints": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional constraints to keep in mind"
                    }
                },
                "required": ["label", "rationale"]
            }),
        ),
        tool_def(
            "read_repo_file",
            "Read a UTF-8 file inside the active Git repository with line numbers (strictly read-only; blocks .git internals and .env files).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Repository-relative path, e.g. 'src/forecast.py' or 'relay.toml'" }
                },
                "required": ["path"]
            }),
        ),
        tool_def(
            "git_diff",
            "Inspect `git status --short` and uncommitted `git diff HEAD` in the active repository (strictly read-only).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
    ]
}

fn tool_def(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl LlmClient {
    /// Stream a chat completion turn with OpenAI tool definitions, invoking `on_delta` per text chunk.
    /// Automatically handles both SSE (`text/event-stream`) and non-streaming JSON responses.
    pub async fn stream_chat_with_tools<F>(
        &self,
        messages: &[Value],
        tools: &[Value],
        cancel: &AtomicBool,
        mut on_delta: F,
    ) -> RelayResult<StreamedChatTurn>
    where
        F: FnMut(&str) + Send,
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(RelayError::Llm("cancelled".into()));
        }

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "temperature": 0.1,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = json!("auto");
        }

        let mut request = self.client.post(self.endpoint.clone()).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let mut response = request.send().await.map_err(super::llm_error)?;
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            // Some OpenAI-compatible gateways reject `stream: true` when `tools` are supplied; retry once non-streaming.
            if status.as_u16() == 400 && !tools.is_empty() {
                return self
                    .non_streaming_chat_with_tools(messages, tools, cancel, &mut on_delta)
                    .await;
            }
            let detail: String = err_body.chars().take(280).collect();
            return Err(RelayError::Llm(format!("HTTP {status}: {detail}")));
        }

        let is_sse = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"));

        if !is_sse {
            let payload: Value = response.json().await.map_err(super::llm_error)?;
            return parse_non_streaming_turn(&payload, &mut on_delta);
        }

        let mut content = String::new();
        let mut partials: BTreeMap<usize, PartialToolCall> = BTreeMap::new();
        let mut buffer = String::new();

        while let Some(bytes) = response.chunk().await.map_err(super::llm_error)? {
            if cancel.load(Ordering::Relaxed) {
                return Err(RelayError::Llm("cancelled".into()));
            }
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_owned();
                buffer.drain(..=newline_pos);
                process_sse_line(&line, &mut content, &mut partials, &mut on_delta);
            }
        }
        if !buffer.trim().is_empty() {
            process_sse_line(buffer.trim(), &mut content, &mut partials, &mut on_delta);
        }

        let tool_calls = partials
            .into_values()
            .filter(|call| !call.name.trim().is_empty())
            .enumerate()
            .map(|(idx, call)| LlmToolCall {
                id: if call.id.is_empty() {
                    format!("call_{idx}")
                } else {
                    call.id
                },
                name: call.name,
                arguments: if call.arguments.trim().is_empty() {
                    "{}".to_owned()
                } else {
                    call.arguments
                },
            })
            .collect();

        Ok(StreamedChatTurn {
            content,
            tool_calls,
        })
    }

    async fn non_streaming_chat_with_tools<F>(
        &self,
        messages: &[Value],
        tools: &[Value],
        cancel: &AtomicBool,
        on_delta: &mut F,
    ) -> RelayResult<StreamedChatTurn>
    where
        F: FnMut(&str) + Send,
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(RelayError::Llm("cancelled".into()));
        }
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "temperature": 0.1,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = json!("auto");
        }
        let mut request = self.client.post(self.endpoint.clone()).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(super::llm_error)?;
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await.unwrap_or_default();
            let detail: String = err_body.chars().take(280).collect();
            return Err(RelayError::Llm(format!("HTTP {status}: {detail}")));
        }
        let payload: Value = response.json().await.map_err(super::llm_error)?;
        parse_non_streaming_turn(&payload, on_delta)
    }
}

fn process_sse_line<F>(
    line: &str,
    content: &mut String,
    partials: &mut BTreeMap<usize, PartialToolCall>,
    on_delta: &mut F,
) where
    F: FnMut(&str),
{
    let Some(data) = line.strip_prefix("data:") else {
        return;
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    let Ok(chunk) = serde_json::from_str::<Value>(data) else {
        return;
    };
    let Some(delta) = chunk
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("delta"))
    else {
        return;
    };

    if let Some(text) = delta.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        content.push_str(text);
        on_delta(text);
    }

    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for (pos, call) in calls.iter().enumerate() {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(pos);
            let entry = partials.entry(index).or_default();
            if let Some(id) = call.get("id").and_then(Value::as_str)
                && !id.is_empty()
            {
                entry.id = id.to_owned();
            }
            if let Some(func) = call.get("function") {
                if let Some(name) = func.get("name").and_then(Value::as_str) {
                    entry.name.push_str(name);
                }
                if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(args);
                }
            }
        }
    }
}

fn parse_non_streaming_turn<F>(payload: &Value, on_delta: &mut F) -> RelayResult<StreamedChatTurn>
where
    F: FnMut(&str),
{
    let message = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| RelayError::Llm("completion response missing choices[0].message".into()))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !content.is_empty() {
        on_delta(&content);
    }

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (idx, call) in calls.iter().enumerate() {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("call_{idx}"));
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let arguments = match call.get("function").and_then(|f| f.get("arguments")) {
                Some(Value::String(raw)) if !raw.trim().is_empty() => raw.clone(),
                Some(other @ Value::Object(_)) => other.to_string(),
                _ => "{}".to_owned(),
            };
            if !name.is_empty() {
                tool_calls.push(LlmToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }
    }

    Ok(StreamedChatTurn {
        content,
        tool_calls,
    })
}
