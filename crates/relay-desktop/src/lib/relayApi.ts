import {
  commands,
  events,
  type AgentChatMessage_Serialize,
  type AgentChatStreamEvent_Serialize,
  type AgentToolTrace_Serialize,
  type ApprovalDecision,
  type DagSnapshot_Serialize,
  type DagStaleAlertEvent,
  type GraphEdge,
  type GraphNode,
  type HandoffSummary,
  type LlmProbeResult,
  type LlmSettingsUpdate_Deserialize,
  type LlmSettingsView_Serialize,
  type PeerStatus_Serialize,
  type RelayStateChangedEvent,
  type RuntimeMode,
  type StalePropagationReport,
  type TaskLogSnapshot,
  type TaskLogStreamEvent,
  type TaskRecord_Serialize,
  type TaskSpec_Deserialize,
  type WorkspaceInfo_Serialize,
} from "../bindings";
import { isTaskCancellable } from "./taskLogStream";

export interface ApprovalInput {
  taskId: string;
  patchId: string | null;
  decision: ApprovalDecision;
}

export function needsTaskApproval(task: TaskRecord_Serialize): boolean {
  return (
    (task.state === "self_healing_proposed" && task.active_proposal !== null) ||
    (task.state === "preflight_running" &&
      task.spec.require_approval &&
      task.preflight_summary === "Smoke preflight passed" &&
      task.active_proposal === null)
  );
}

export function requireCurrentApproval(task: TaskRecord_Serialize, patchId: string | null): void {
  if ((task.active_proposal?.patch_id ?? null) !== patchId) {
    throw new Error("The suggested fix has changed. Review the current fix before continuing.");
  }
  if (!needsTaskApproval(task)) throw new Error("This task is not awaiting approval.");
}

export function findCommitArtifact(dag: DagSnapshot_Serialize, commit: string | null) {
  const candidates = dag.nodes.filter(
    (node) => node.payload.kind === "artifact" && node.payload.git_commit_sha === commit,
  );
  const superseded = new Set(
    dag.edges.filter((edge) => edge.kind === "supersedes").map((edge) => edge.to),
  );
  return (
    candidates.sort(
      (a, b) =>
        Number(superseded.has(a.id)) - Number(superseded.has(b.id)) ||
        Date.parse(b.updated_at) - Date.parse(a.updated_at) ||
        Date.parse(b.created_at) - Date.parse(a.created_at) ||
        b.id.localeCompare(a.id),
    )[0] ?? null
  );
}

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface MockWorkspaceState {
  status: PeerStatus_Serialize;
  workspace: WorkspaceInfo_Serialize;
  llm: LlmSettingsView_Serialize;
  dag: DagSnapshot_Serialize;
  tasks: TaskRecord_Serialize[];
  taskLogs: Record<string, TaskLogSnapshot>;
  handoffs: HandoffSummary[];
  chatMessages: AgentChatMessage_Serialize[];
}

function createInitialMockState(): MockWorkspaceState {
  const now = new Date().toISOString();
  return {
    status: {
      mode: "local_dual_sandbox",
      local_node_id: "local-preview",
      remote_node_id: null,
      transport_path: "offline",
      connected: false,
      rtt_ms: null,
      last_synced_commit: null,
      pending_outbox_count: 0,
    },
    workspace: {
      repo_id: "default-workspace",
      name: "",
      repo_root: null,
      mcp_data_dir: null,
      is_git_repo: false,
      has_manifest: false,
      smoke_preset: "",
      presets: [],
    },
    llm: {
      tier: "host_local_ollama",
      base_url: "http://127.0.0.1:11434/v1",
      model: "qwen3.5:4b",
      api_key_env: null,
      has_api_key: false,
      api_key_hint: null,
      reasoning_effort: null,
    },
    dag: {
      repo_id: "default-workspace",
      head_commit_sha: null,
      nodes: [],
      edges: [],
      stale_count: 0,
      fresh_evidence_count: 0,
      stale_evidence_count: 0,
      metric_authority_note: null,
      captured_at: now,
    },
    tasks: [],
    taskLogs: {},
    handoffs: [],
    chatMessages: [],
  };
}

let mockState: MockWorkspaceState = createInitialMockState();
const previewWorkspaces = new Map<string, MockWorkspaceState>();
const mockChatListeners = new Set<(event: AgentChatStreamEvent_Serialize) => void>();
const mockTaskLogListeners = new Set<(event: TaskLogStreamEvent) => void>();
let activeMockChatAbort: { cancelled: boolean } | null = null;

function emitMockChatEvent(event: AgentChatStreamEvent_Serialize): void {
  for (const listener of mockChatListeners) {
    listener(event);
  }
}

export function emitPreviewTaskLogEvent(event: TaskLogStreamEvent): void {
  const existing = mockState.taskLogs[event.task_id];
  const content = (existing?.content ?? "") + event.text;
  mockState.taskLogs[event.task_id] = {
    task_id: event.task_id,
    log_blob_hash: existing?.log_blob_hash ?? `preview-${event.task_id.slice(0, 8)}`,
    content,
  };
  for (const listener of mockTaskLogListeners) {
    listener(event);
  }
}

function sleepMs(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function hasCjk(text: string): boolean {
  return /[\u3400-\u9fff]/.test(text);
}

async function runMockCopilotTurn(
  prompt: string,
  assistantMsg: AgentChatMessage_Serialize,
  abort: { cancelled: boolean },
): Promise<void> {
  const turnId = `mock-turn-${Date.now()}`;
  const isZh = hasCjk(prompt);
  const lower = prompt.toLowerCase();

  emitMockChatEvent({
    turn_id: turnId,
    message_id: assistantMsg.id,
    kind: "turn_started",
    delta: null,
    trace: null,
    approval: null,
    error: null,
  });

  const runTool = async (
    toolName: string,
    argsPreview: string,
    computeOutput: () => string,
  ): Promise<void> => {
    if (abort.cancelled) return;
    const trace: AgentToolTrace_Serialize = {
      id: `${toolName}-${assistantMsg.traces.length}`,
      tool_name: toolName,
      args_preview: argsPreview,
      status: "running",
      output_preview: null,
      duration_ms: null,
    };
    assistantMsg.traces.push(trace);
    emitMockChatEvent({
      turn_id: turnId,
      message_id: assistantMsg.id,
      kind: "tool_started",
      delta: null,
      trace: structuredClone(trace),
      approval: null,
      error: null,
    });

    await sleepMs(180);
    if (abort.cancelled) return;

    const output = computeOutput();
    trace.status = "done";
    trace.output_preview = output;
    trace.duration_ms = 12 + assistantMsg.traces.length * 7;
    emitMockChatEvent({
      turn_id: turnId,
      message_id: assistantMsg.id,
      kind: "tool_finished",
      delta: null,
      trace: structuredClone(trace),
      approval: null,
      error: null,
    });
  };

  let responseText = "";
  const presetNames = mockState.workspace.presets.map((p) => p.name);
  const hasRepo = Boolean(mockState.workspace.repo_root);
  const shortHead = (mockState.dag.head_commit_sha ?? "HEAD").slice(0, 7);

  if (lower.includes("outdated") || lower.includes("stale") || prompt.includes("过期")) {
    await runTool("get_dag_snapshot", "{}", () =>
      JSON.stringify(
        {
          repo_id: mockState.dag.repo_id,
          head_commit_sha: mockState.dag.head_commit_sha,
          stale_count: mockState.dag.stale_count,
          fresh_evidence_count: mockState.dag.fresh_evidence_count,
          stale_evidence_count: mockState.dag.stale_evidence_count,
          node_count: mockState.dag.nodes.length,
        },
        null,
        2,
      ),
    );
    const staleNodes = mockState.dag.nodes.filter((n) => n.freshness.status === "stale");
    responseText =
      staleNodes.length > 0
        ? isZh
          ? `检测到 **${staleNodes.length} 项过期结果**（当前提交 \`${shortHead}\`），旧指标已自动隔离。\n\n建议在 \`relay.toml\` 中选择预设重新执行。`
          : `Found **${staleNodes.length} outdated result(s)** against HEAD \`${shortHead}\`. Quarantined metrics are excluded from reasoning.\n\nRe-run a preset from \`relay.toml\` to record fresh results.`
        : isZh
          ? `当前没有过期结果（\`stale_count = 0\`）。请先打开包含 \`relay.toml\` 的本地 Git 仓库，或运行首个预设任务。`
          : `No outdated results right now (\`stale_count = 0\`). Open a local Git repository with \`relay.toml\` or run your first preset task.`;
  } else if (
    lower.includes("env") ||
    lower.includes("sync") ||
    prompt.includes("环境") ||
    prompt.includes("同步")
  ) {
    await runTool("inspect_sync_status", "{}", () => JSON.stringify(mockState.status, null, 2));
    if (!abort.cancelled && hasRepo) {
      await runTool("check_env_diff", '{"commit_sha":"HEAD"}', () =>
        JSON.stringify(
          {
            commit_sha: mockState.dag.head_commit_sha ?? "HEAD",
            ecosystem: "generic",
            package_manager: "generic",
            in_sync: true,
            missing_packages: [],
          },
          null,
          2,
        ),
      );
    }
    responseText = isZh
      ? `### 连接与环境概览\n\n- **连接状态**：${mockState.status.connected ? "已连接" : "未连接远程主机"}（\`${mockState.status.transport_path}\`）\n- **当前仓库**：${hasRepo ? mockState.workspace.name || mockState.workspace.repo_id : "未打开本地仓库"}\n- **当前预设**：${presetNames.length > 0 ? presetNames.map((n) => `\`${n}\``).join("、") : "暂无"}`
      : `### Sync & Environment Summary\n\n- **Link**: ${mockState.status.connected ? "Connected" : "Offline"} (\`${mockState.status.transport_path}\`)\n- **Repository**: ${hasRepo ? mockState.workspace.name || mockState.workspace.repo_id : "No repository selected"}\n- **Configured presets**: ${presetNames.length > 0 ? presetNames.map((n) => `\`${n}\``).join(", ") : "none"}`;
  } else {
    await runTool("get_dag_snapshot", "{}", () =>
      JSON.stringify(
        {
          repo_id: mockState.dag.repo_id,
          head_commit_sha: mockState.dag.head_commit_sha,
          nodes: mockState.dag.nodes.length,
          stale_count: mockState.dag.stale_count,
          fresh_evidence_count: mockState.dag.fresh_evidence_count,
          stale_evidence_count: mockState.dag.stale_evidence_count,
          presets: presetNames,
        },
        null,
        2,
      ),
    );
    responseText = isZh
      ? hasRepo
        ? `当前工作区 **${mockState.workspace.name || mockState.workspace.repo_id}**（HEAD \`${shortHead}\`）共有 **${mockState.dag.nodes.length}** 个图谱节点，可用预设：${presetNames.length > 0 ? presetNames.map((n) => `\`${n}\``).join("、") : "请先配置 `relay.toml`"}。`
        : "当前尚未打开本地仓库。请在顶部或工作区设置中选择本地 Git 仓库，或者配对一台远程主机。"
      : hasRepo
        ? `Workspace **${mockState.workspace.name || mockState.workspace.repo_id}** (HEAD \`${shortHead}\`) has **${mockState.dag.nodes.length}** graph nodes and presets: ${presetNames.length > 0 ? presetNames.map((n) => `\`${n}\``).join(", ") : "generate `relay.toml` to add presets"}.`
        : "No local repository is open yet. Open a local Git repository or pair with a remote host to get started.";
  }

  const chunks = responseText.match(/.{1,12}/gs) ?? [responseText];
  for (const chunk of chunks) {
    if (abort.cancelled) break;
    assistantMsg.content += chunk;
    emitMockChatEvent({
      turn_id: turnId,
      message_id: assistantMsg.id,
      kind: "token_delta",
      delta: chunk,
      trace: null,
      approval: null,
      error: null,
    });
    await sleepMs(16);
  }

  if (activeMockChatAbort !== abort) return;
  activeMockChatAbort = null;

  emitMockChatEvent({
    turn_id: turnId,
    message_id: assistantMsg.id,
    kind: "turn_completed",
    delta: null,
    trace: null,
    approval: null,
    error: null,
  });
}

export function resetBrowserPreviewState(): void {
  if (activeMockChatAbort) {
    activeMockChatAbort.cancelled = true;
    activeMockChatAbort = null;
  }
  mockState = createInitialMockState();
  previewWorkspaces.clear();
}

export const relayApi = {
  async getRuntimeStatus(): Promise<PeerStatus_Serialize> {
    if (isTauriRuntime()) {
      return commands.getRuntimeStatus();
    }
    return structuredClone(mockState.status);
  },

  async getWorkspaceInfo(): Promise<WorkspaceInfo_Serialize> {
    if (isTauriRuntime()) {
      return commands.getWorkspaceInfo();
    }
    return structuredClone(mockState.workspace);
  },

  async pickRepositoryFolder(): Promise<string | null> {
    if (isTauriRuntime()) {
      return commands.pickRepositoryFolder();
    }
    return null;
  },

  async openWorkspaceRepo(repoPath: string): Promise<WorkspaceInfo_Serialize> {
    if (isTauriRuntime()) {
      return commands.openWorkspaceRepo(repoPath);
    }
    const trimmed = repoPath.trim();
    if (!trimmed) {
      throw new Error("Repository path cannot be empty.");
    }
    if (mockState.workspace.repo_root === trimmed) return structuredClone(mockState.workspace);
    if (activeMockChatAbort) {
      activeMockChatAbort.cancelled = true;
      activeMockChatAbort = null;
    }
    if (mockState.workspace.repo_root)
      previewWorkspaces.set(mockState.workspace.repo_root, mockState);
    const saved = previewWorkspaces.get(trimmed);
    const llm = mockState.llm;
    mockState = saved ?? createInitialMockState();
    mockState.llm = llm;
    if (saved) return structuredClone(mockState.workspace);
    const slug = trimmed.split(/[\\/]/).filter(Boolean).pop() ?? "workspace";
    mockState.workspace = {
      repo_id: slug,
      name: slug,
      repo_root: trimmed,
      mcp_data_dir: null,
      is_git_repo: true,
      has_manifest: false,
      smoke_preset: "",
      presets: [],
    };
    mockState.dag.repo_id = slug;
    return structuredClone(mockState.workspace);
  },

  async initWorkspaceManifest(repoPath?: string | null): Promise<WorkspaceInfo_Serialize> {
    if (isTauriRuntime()) {
      return commands.initWorkspaceManifest(repoPath ?? null);
    }
    const effectivePath = (repoPath ?? mockState.workspace.repo_root ?? "").trim();
    if (!effectivePath) {
      throw new Error("Open a local Git repository first.");
    }
    await relayApi.openWorkspaceRepo(effectivePath);
    if (mockState.workspace.has_manifest) throw new Error("relay.toml already exists.");
    const slug = effectivePath.split(/[\\/]/).filter(Boolean).pop() ?? "workspace";
    mockState.workspace = {
      repo_id: slug,
      name: slug,
      repo_root: effectivePath,
      mcp_data_dir: null,
      is_git_repo: true,
      has_manifest: true,
      smoke_preset: "check",
      presets: [
        {
          name: "check",
          description: "Quick workspace sanity check",
          program: "git",
          args: ["status", "--short"],
          allowed_params: [],
        },
      ],
    };
    return structuredClone(mockState.workspace);
  },

  async refreshGitHead(): Promise<boolean> {
    if (isTauriRuntime()) {
      return commands.refreshGitHead();
    }
    return false;
  },

  async getLlmSettings(): Promise<LlmSettingsView_Serialize> {
    if (isTauriRuntime()) {
      return commands.getLlmSettings();
    }
    return structuredClone(mockState.llm);
  },

  async saveLlmSettings(update: LlmSettingsUpdate_Deserialize): Promise<LlmSettingsView_Serialize> {
    if (isTauriRuntime()) {
      return commands.saveLlmSettings(update);
    }
    const nextKey = update.api_key;
    const hasKey =
      nextKey === undefined || nextKey === null
        ? mockState.llm.has_api_key
        : nextKey.trim().length > 0;
    const hint =
      nextKey && nextKey.trim().length > 0
        ? `${nextKey.trim().slice(0, 3)}••••${nextKey.trim().slice(-4)}`
        : nextKey === ""
          ? null
          : mockState.llm.api_key_hint;
    mockState.llm = {
      tier: update.tier,
      base_url: update.base_url.trim(),
      model: update.model.trim(),
      api_key_env: update.api_key_env?.trim() || null,
      has_api_key: hasKey || Boolean(update.api_key_env?.trim()),
      api_key_hint: hint,
      reasoning_effort: update.reasoning_effort?.trim() || null,
    };
    return structuredClone(mockState.llm);
  },

  async testLlmConnection(update: LlmSettingsUpdate_Deserialize | null): Promise<LlmProbeResult> {
    if (isTauriRuntime()) {
      return commands.testLlmConnection(update);
    }
    const base = update?.base_url ?? mockState.llm.base_url;
    const model = update?.model ?? mockState.llm.model;
    return {
      ok: true,
      latency_ms: 14,
      endpoint: base,
      model,
      message: `Preview OK (14 ms) — run in Tauri desktop to probe live ${base}`,
    };
  },

  async switchRuntimeMode(mode: RuntimeMode): Promise<RuntimeMode> {
    if (isTauriRuntime()) {
      return commands.switchRuntimeMode(mode);
    }
    mockState.status.mode = mode;
    mockState.status.transport_path =
      mode === "local_dual_sandbox" ? "loopback" : mode === "client" ? "relay" : "direct_lan";
    mockState.status.rtt_ms = mode === "local_dual_sandbox" ? 1 : mode === "client" ? 18 : 2;
    return mode;
  },

  async getDagSnapshot(includeStale: boolean): Promise<DagSnapshot_Serialize> {
    if (isTauriRuntime()) {
      return commands.getDagSnapshot(includeStale);
    }
    const staleCount = mockState.dag.nodes.filter((n) => n.freshness.status === "stale").length;
    const freshEvidenceCount = mockState.dag.nodes.filter(
      (n) => n.payload.kind === "evidence" && n.freshness.status === "fresh",
    ).length;
    const staleEvidenceCount = mockState.dag.nodes.filter(
      (n) => n.payload.kind === "evidence" && n.freshness.status === "stale",
    ).length;
    const nodes = includeStale
      ? mockState.dag.nodes
      : mockState.dag.nodes.filter((n) => n.freshness.status !== "stale");
    const allowed = new Set(nodes.map((n) => n.id));
    const edges = mockState.dag.edges.filter((e) => allowed.has(e.from) && allowed.has(e.to));
    return structuredClone({
      ...mockState.dag,
      nodes,
      edges,
      stale_count: staleCount,
      fresh_evidence_count: freshEvidenceCount,
      stale_evidence_count: staleEvidenceCount,
      metric_authority_note:
        freshEvidenceCount === 0 && staleEvidenceCount > 0
          ? "WARNING: Current HEAD has 0 fresh verified evidence nodes and all prior evidence is STALE/quarantined."
          : null,
      captured_at: new Date().toISOString(),
    });
  },

  async listTasks(): Promise<TaskRecord_Serialize[]> {
    if (isTauriRuntime()) {
      return commands.listTasks();
    }
    return structuredClone(mockState.tasks);
  },

  async listHandoffs(): Promise<HandoffSummary[]> {
    if (isTauriRuntime()) {
      return commands.listHandoffs();
    }
    return structuredClone(mockState.handoffs);
  },

  async submitTask(spec: TaskSpec_Deserialize): Promise<TaskRecord_Serialize> {
    if (isTauriRuntime()) {
      return commands.submitTask(spec);
    }
    const now = new Date().toISOString();
    const record: TaskRecord_Serialize = {
      spec: {
        ...spec,
        task_id: spec.task_id || crypto.randomUUID(),
        user_note: spec.user_note ?? null,
      },
      state: "queued",
      active_proposal: null,
      preflight_summary: "Waiting for host pre-check",
      evidence_node_id: null,
      error_message: null,
      created_at: now,
      updated_at: now,
    };
    mockState.tasks = [record, ...mockState.tasks];
    return structuredClone(record);
  },

  async cancelTask(taskId: string): Promise<TaskRecord_Serialize> {
    if (isTauriRuntime()) {
      return commands.cancelTask(taskId);
    }
    const task = mockState.tasks.find((t) => t.spec.task_id === taskId);
    if (!task) {
      throw new Error(`task not found: ${taskId}`);
    }
    if (!isTaskCancellable(task.state)) {
      throw new Error("This task can no longer be cancelled.");
    }
    task.state = "cancelled";
    task.preflight_summary = mockState.taskLogs[taskId]?.content
      ? "Cancelled by user (partial log saved)"
      : "Cancelled by user";
    task.error_message = null;
    task.updated_at = new Date().toISOString();
    return structuredClone(task);
  },

  async getTaskLog(taskId: string): Promise<TaskLogSnapshot | null> {
    if (isTauriRuntime()) {
      return commands.getTaskLog(taskId);
    }
    const log = mockState.taskLogs[taskId];
    return log ? structuredClone(log) : null;
  },

  async resolvePatchApproval({
    taskId,
    patchId,
    decision,
  }: ApprovalInput): Promise<TaskRecord_Serialize> {
    if (isTauriRuntime()) {
      return commands.resolvePatchApproval(taskId, patchId, decision);
    }
    const task = mockState.tasks.find((t) => t.spec.task_id === taskId);
    if (!task) throw new Error(`Task not found: ${taskId}`);
    requireCurrentApproval(task, patchId);
    const now = new Date().toISOString();
    if (decision === "approved") {
      task.state = "completed";
      task.updated_at = now;
      task.preflight_summary = "Fix applied and run finished cleanly.";
      const evidenceId = `ev-${taskId.slice(0, 8)}`;
      task.evidence_node_id = evidenceId;
      const baseArtifact = findCommitArtifact(mockState.dag, task.spec.base_commit_sha);
      if (!mockState.dag.nodes.some((n) => n.id === evidenceId)) {
        mockState.dag.nodes.push({
          id: evidenceId,
          label: `${task.spec.command_preset} result`,
          payload: {
            kind: "evidence",
            task_id: task.spec.task_id,
            preset: task.spec.command_preset,
            metrics_json: {
              status: "ok",
              elapsed_ms: 42,
            },
            log_blob_hash: "blake3:preview-log",
          },
          freshness: { status: "fresh" },
          device_origin: "Workstation",
          created_at: now,
          updated_at: now,
        });
        if (baseArtifact) {
          mockState.dag.edges.push({
            id: `edge-${baseArtifact.id}-${evidenceId}`,
            from: baseArtifact.id,
            to: evidenceId,
            kind: "produced_from",
          });
        }
      }
    } else {
      task.state = "rejected";
      task.updated_at = now;
      task.preflight_summary = "Suggested fix dismissed.";
    }
    for (const msg of mockState.chatMessages) {
      if (msg.approval && msg.approval.task_id === taskId && msg.approval.patch_id === patchId) {
        msg.approval.resolved = decision;
      }
    }
    return structuredClone(task);
  },

  async simulateArtifactUpdate(
    oldArtifactId: string,
    newCommitSha: string,
    reason: string,
  ): Promise<StalePropagationReport> {
    if (isTauriRuntime()) {
      return commands.simulateArtifactUpdate(oldArtifactId, newCommitSha, reason);
    }
    const now = new Date().toISOString();
    const newId = `art-${newCommitSha.slice(0, 7)}`;
    if (!mockState.dag.nodes.some((n) => n.id === newId)) {
      mockState.dag.nodes.push({
        id: newId,
        label: `Commit ${newCommitSha.slice(0, 7)}`,
        payload: {
          kind: "artifact",
          git_commit_sha: newCommitSha,
          env_fingerprint: "blake3:preview",
          working_tree_dirty: true,
          changed_files: [],
        },
        freshness: { status: "fresh" },
        device_origin: "Laptop",
        created_at: now,
        updated_at: now,
      });
      mockState.dag.edges.push({
        id: `supersedes:${newId}->${oldArtifactId}`,
        from: newId,
        to: oldArtifactId,
        kind: "supersedes",
      });
    }

    const nodeById = new Map(mockState.dag.nodes.map((n) => [n.id, n]));
    const visited = new Set<string>([newId]);
    const queue = [newId];
    const invalidated: string[] = [];

    while (queue.length > 0) {
      const current = queue.shift()!;
      for (const edge of mockState.dag.edges) {
        if (edge.from === current && !visited.has(edge.to)) {
          visited.add(edge.to);
          queue.push(edge.to);
          const target = nodeById.get(edge.to);
          if (target && target.payload.kind === "evidence") {
            target.freshness = {
              status: "stale",
              caused_by: newId,
              reason,
              marked_at: now,
            };
            target.updated_at = now;
            invalidated.push(target.id);
          }
        }
      }
    }

    mockState.dag.head_commit_sha = newCommitSha;
    mockState.dag.stale_count = mockState.dag.nodes.filter(
      (n) => n.freshness.status === "stale",
    ).length;
    mockState.status.last_synced_commit = newCommitSha.slice(0, 7);

    return {
      trigger_node_id: newId,
      invalidated_evidence_ids: invalidated,
      reason,
    };
  },

  async upsertDagNode(node: GraphNode, edges: GraphEdge[]): Promise<DagSnapshot_Serialize> {
    if (isTauriRuntime()) {
      return commands.upsertDagNode(node, edges);
    }
    const existingIdx = mockState.dag.nodes.findIndex((n) => n.id === node.id);
    if (existingIdx >= 0) {
      mockState.dag.nodes[existingIdx] = node;
    } else {
      mockState.dag.nodes.push(node);
    }
    for (const edge of edges) {
      if (!mockState.dag.edges.some((e) => e.id === edge.id)) {
        mockState.dag.edges.push(edge);
      }
    }
    return this.getDagSnapshot(true);
  },

  async getPairingTicket(): Promise<string> {
    if (isTauriRuntime()) {
      return commands.getPairingTicket();
    }
    return JSON.stringify(
      {
        id: mockState.status.local_node_id,
        addrs: ["192.168.1.42:49152"],
      },
      null,
      2,
    );
  },

  async connectPeerTicket(ticketJson: string): Promise<PeerStatus_Serialize> {
    if (isTauriRuntime()) {
      return commands.connectPeerTicket(ticketJson);
    }
    const parsed = JSON.parse(ticketJson) as { id?: string };
    mockState.status.remote_node_id = parsed.id ?? "paired-peer";
    mockState.status.connected = true;
    mockState.status.transport_path = "direct_lan";
    mockState.status.rtt_ms = 2;
    return structuredClone(mockState.status);
  },

  async listChatMessages(limit = 100): Promise<AgentChatMessage_Serialize[]> {
    if (isTauriRuntime()) {
      return commands.listChatMessages(limit);
    }
    return structuredClone(mockState.chatMessages.slice(-limit));
  },

  async sendChatMessage(prompt: string): Promise<AgentChatMessage_Serialize> {
    if (isTauriRuntime()) {
      return commands.sendChatMessage(prompt);
    }
    const trimmed = prompt.trim();
    if (!trimmed) {
      throw new Error("Message cannot be empty.");
    }
    if (activeMockChatAbort) {
      activeMockChatAbort.cancelled = true;
    }
    const now = Date.now();
    const userMsg: AgentChatMessage_Serialize = {
      id: `user-${now}`,
      repo_id: mockState.workspace.repo_id,
      role: "user",
      content: trimmed,
      traces: [],
      approval: null,
      error: null,
      created_at: new Date(now).toISOString(),
    };
    const assistantMsg: AgentChatMessage_Serialize = {
      id: `assistant-${now + 1}`,
      repo_id: mockState.workspace.repo_id,
      role: "assistant",
      content: "",
      traces: [],
      approval: null,
      error: null,
      created_at: new Date(now + 1).toISOString(),
    };
    mockState.chatMessages.push(userMsg, assistantMsg);
    const abort = { cancelled: false };
    activeMockChatAbort = abort;
    void runMockCopilotTurn(trimmed, assistantMsg, abort);
    return structuredClone(assistantMsg);
  },

  async cancelChatTurn(): Promise<boolean> {
    if (isTauriRuntime()) {
      return commands.cancelChatTurn();
    }
    if (activeMockChatAbort) {
      activeMockChatAbort.cancelled = true;
      activeMockChatAbort = null;
      return true;
    }
    return false;
  },

  async clearChatMessages(): Promise<void> {
    if (isTauriRuntime()) {
      await commands.clearChatMessages();
      return;
    }
    if (activeMockChatAbort) {
      activeMockChatAbort.cancelled = true;
      activeMockChatAbort = null;
    }
    mockState.chatMessages = [];
  },

  async subscribeAgentChatEvents(
    onEvent: (payload: AgentChatStreamEvent_Serialize) => void,
  ): Promise<() => void> {
    if (isTauriRuntime()) {
      return events.agentChatStreamEvent.listen((ev) => onEvent(ev.payload));
    }
    mockChatListeners.add(onEvent);
    return () => {
      mockChatListeners.delete(onEvent);
    };
  },

  async subscribeTaskLogEvents(
    onEvent: (payload: TaskLogStreamEvent) => void,
  ): Promise<() => void> {
    if (isTauriRuntime()) {
      return events.taskLogStreamEvent.listen((ev) => onEvent(ev.payload));
    }
    mockTaskLogListeners.add(onEvent);
    return () => {
      mockTaskLogListeners.delete(onEvent);
    };
  },

  async subscribeEvents(handlers: {
    onStaleAlert?: (payload: DagStaleAlertEvent) => void;
    onStateChanged?: (payload: RelayStateChangedEvent) => void;
  }): Promise<() => void> {
    if (!isTauriRuntime()) {
      return () => {};
    }
    const unlisteners: Array<() => void> = [];
    if (handlers.onStaleAlert) {
      unlisteners.push(
        await events.dagStaleAlertEvent.listen((ev) => handlers.onStaleAlert?.(ev.payload)),
      );
    }
    if (handlers.onStateChanged) {
      unlisteners.push(
        await events.relayStateChangedEvent.listen((ev) => handlers.onStateChanged?.(ev.payload)),
      );
    }
    return () => {
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  },
};
