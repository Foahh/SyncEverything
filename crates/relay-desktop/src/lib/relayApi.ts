import {
  commands,
  events,
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
  type TaskRecord_Serialize,
  type TaskSpec_Deserialize,
  type WorkspaceInfo_Serialize,
} from "../bindings";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface MockWorkspaceState {
  status: PeerStatus_Serialize;
  workspace: WorkspaceInfo_Serialize;
  llm: LlmSettingsView_Serialize;
  dag: DagSnapshot_Serialize;
  tasks: TaskRecord_Serialize[];
  handoffs: HandoffSummary[];
}

function createInitialMockState(): MockWorkspaceState {
  const t0 = "2026-11-21T14:10:00Z";
  const t1 = "2026-11-21T14:16:00Z";
  const t2 = "2026-11-21T14:22:00Z";
  const t3 = "2026-11-21T14:28:00Z";

  const nodes: GraphNode[] = [
    {
      id: "dec-window-strategy",
      label: "Switch to hybrid MA + EWMA forecast",
      payload: {
        kind: "decision",
        rationale:
          "Blend moving average stability with EWMA responsiveness across 7/14/28 day windows, and offload the full sweep to the desktop GPU.",
        constraints: ["Keep pre-check under 2s", "Pin packages in uv.lock"],
      },
      freshness: { status: "fresh" },
      device_origin: "MacBook Air",
      created_at: t0,
      updated_at: t0,
    },
    {
      id: "art-a91f0c2",
      label: "Add seasonal weighting",
      payload: {
        kind: "artifact",
        git_commit_sha: "a91f0c28e4b71a2d9012f",
        env_fingerprint: "uv:blake3:9c8f4a21",
        working_tree_dirty: false,
        changed_files: ["src/forecast.py", "relay.toml", "pyproject.toml"],
      },
      freshness: { status: "fresh" },
      device_origin: "MacBook Air",
      created_at: t1,
      updated_at: t1,
    },
    {
      id: "ev-grid-7-14-28",
      label: "Window sweep (7, 14, 28)",
      payload: {
        kind: "evidence",
        task_id: "019a9e80-41f2-7d3a-9c44-81b2f3001001",
        preset: "train_grid",
        metrics_json: {
          best_mae: 2.165,
          best_window: 28,
          rmse_w28: 2.568,
          alpha: 0.35,
          elapsed_ms: 1.08,
        },
        log_blob_hash: "blake3:7a31d908f42b1c90e442",
      },
      freshness: { status: "fresh" },
      device_origin: "Windows Workstation",
      created_at: t2,
      updated_at: t2,
    },
  ];

  const edges: GraphEdge[] = [
    {
      id: "edge-dec-art",
      from: "dec-window-strategy",
      to: "art-a91f0c2",
      kind: "informs",
    },
    {
      id: "edge-art-ev",
      from: "art-a91f0c2",
      to: "ev-grid-7-14-28",
      kind: "produced_from",
    },
  ];

  const tasks: TaskRecord_Serialize[] = [
    {
      spec: {
        task_id: "019a9e80-78c1-7a91-b302-9a10e44b2002",
        repo_id: "forecast-bench",
        base_commit_sha: "a91f0c28e4b71a2d9012f",
        command_preset: "train_grid",
        params: { windows: "7,14,28", alpha: "0.45" },
        require_approval: true,
        requested_by_device: "MacBook Air",
        user_note: "Re-run window sweep with alpha=0.45 on workstation.",
      },
      state: "self_healing_proposed",
      active_proposal: {
        patch_id: "patch-uv-scipy-01",
        task_id: "019a9e80-78c1-7a91-b302-9a10e44b2002",
        kind: "uv_spec",
        summary:
          "Missing package `scipy` blocked the run. Added `scipy>=1.15.2` to pyproject.toml and verified a quick check in an isolated copy.",
        diff_preview: [
          "--- a/pyproject.toml",
          "+++ b/pyproject.toml",
          '@@ -4,7 +4,9 @@ version = "0.1.0"',
          ' description = "Time-series window forecasting"',
          ' requires-python = ">=3.11"',
          "-dependencies = []",
          "+dependencies = [",
          '+    "scipy>=1.15.2",',
          "+]",
          " ",
          " [tool.uv]",
          " package = false",
        ].join("\n"),
        worktree_branch: "relay/heal/019a9e80-scipy",
        smoke_verified: true,
        smoke_stdout_tail:
          "[uv] Resolved 2 packages in 18ms (cached)\n[smoke] best_window=7  best_mae=2.464  elapsed=0.24ms\n[ok] Exit code 0",
        created_at: t3,
      },
      preflight_summary: "Missing package `scipy` — fix tested and ready to apply.",
      evidence_node_id: null,
      error_message: null,
      created_at: t2,
      updated_at: t3,
    },
    {
      spec: {
        task_id: "019a9e80-41f2-7d3a-9c44-81b2f3001001",
        repo_id: "forecast-bench",
        base_commit_sha: "a91f0c28e4b71a2d9012f",
        command_preset: "train_grid",
        params: { windows: "7,14,28", alpha: "0.35" },
        require_approval: true,
        requested_by_device: "MacBook Air",
        user_note: "Baseline 3-window sweep.",
      },
      state: "completed",
      active_proposal: null,
      preflight_summary: "Finished cleanly in 1.08ms",
      evidence_node_id: "ev-grid-7-14-28",
      error_message: null,
      created_at: t1,
      updated_at: t2,
    },
  ];

  const handoffs: HandoffSummary[] = [
    {
      handoff_id: "ho-20261121-02",
      repo_id: "forecast-bench",
      source_device: "MacBook Air",
      base_commit_sha: "a91f0c2",
      summary: "Sent window sweep (alpha=0.45) to Workstation",
      active_task_count: 1,
      stale_evidence_count: 0,
      created_at: t3,
    },
    {
      handoff_id: "ho-20261121-01",
      repo_id: "forecast-bench",
      source_device: "Windows Workstation",
      base_commit_sha: "a91f0c2",
      summary: "Completed baseline sweep (best MAE 2.165)",
      active_task_count: 0,
      stale_evidence_count: 0,
      created_at: t2,
    },
  ];

  return {
    status: {
      mode: "local_dual_sandbox",
      local_node_id: "iroh1q8m4k9x7...macbook",
      remote_node_id: "iroh1w5060hx9...winhost",
      transport_path: "direct_lan",
      connected: true,
      rtt_ms: 2,
      last_synced_commit: "a91f0c2",
      pending_outbox_count: 0,
    },
    workspace: {
      repo_id: "forecast-bench",
      name: "Rolling-Window Forecast Fixture",
      repo_root: "bench/fixture-repo",
      is_git_repo: false,
      has_manifest: true,
      smoke_preset: "smoke",
      presets: [
        {
          name: "train_grid",
          description:
            "Evaluate 3 rolling window sizes (7, 14, 28) and emit structured JSON metrics",
          program: "uv",
          args: ["run", "python", "src/forecast.py", "--windows", "7,14,28"],
          allowed_params: ["windows", "alpha"],
        },
        {
          name: "smoke",
          description: "Fast 1-second smoke preflight verifying runtime & synthetic data pipeline",
          program: "uv",
          args: ["run", "python", "src/forecast.py", "--smoke"],
          allowed_params: ["window"],
        },
      ],
    },
    llm: {
      tier: "host_local_ollama",
      base_url: "http://127.0.0.1:11434/v1",
      model: "qwen3.5:4b",
      api_key_env: null,
      has_api_key: false,
      api_key_hint: null,
    },
    dag: {
      repo_id: "forecast-bench",
      head_commit_sha: "a91f0c28e4b71a2d9012f",
      nodes,
      edges,
      stale_count: 0,
      captured_at: t3,
    },
    tasks,
    handoffs,
  };
}

let mockState: MockWorkspaceState = createInitialMockState();

export function resetBrowserPreviewState(): void {
  mockState = createInitialMockState();
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
    const slug = trimmed.split(/[\\/]/).filter(Boolean).pop() ?? "custom-repo";
    mockState.workspace = {
      ...mockState.workspace,
      repo_id: slug,
      name: slug,
      repo_root: trimmed,
      is_git_repo: true,
    };
    mockState.dag.repo_id = slug;
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

  async resolvePatchApproval(
    taskId: string,
    decision: ApprovalDecision,
  ): Promise<TaskRecord_Serialize> {
    if (isTauriRuntime()) {
      return commands.resolvePatchApproval(taskId, decision);
    }
    const task = mockState.tasks.find((t) => t.spec.task_id === taskId);
    if (!task) throw new Error(`Task not found: ${taskId}`);
    const now = new Date().toISOString();
    if (decision === "approved") {
      task.state = "completed";
      task.updated_at = now;
      task.preflight_summary = "Fix applied and run finished cleanly.";
      const evidenceId = `ev-${taskId.slice(0, 8)}`;
      task.evidence_node_id = evidenceId;
      const latestArtifact = [...mockState.dag.nodes]
        .reverse()
        .find((n) => n.payload.kind === "artifact");
      if (!mockState.dag.nodes.some((n) => n.id === evidenceId)) {
        mockState.dag.nodes.push({
          id: evidenceId,
          label: `${task.spec.command_preset} (fixed)`,
          payload: {
            kind: "evidence",
            task_id: task.spec.task_id,
            preset: task.spec.command_preset,
            metrics_json: {
              best_mae: 1.942,
              best_window: 28,
              rmse_w28: 2.31,
              alpha: Number(task.spec.params.alpha ?? 0.45),
              elapsed_ms: 1.14,
            },
            log_blob_hash: "blake3:9e12ca44110b83df2210",
          },
          freshness: { status: "fresh" },
          device_origin: "Windows Workstation",
          created_at: now,
          updated_at: now,
        });
        if (latestArtifact) {
          mockState.dag.edges.push({
            id: `edge-${latestArtifact.id}-${evidenceId}`,
            from: latestArtifact.id,
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
        label: "Update forecast weighting",
        payload: {
          kind: "artifact",
          git_commit_sha: newCommitSha,
          env_fingerprint: "uv:blake3:9c8f4a21",
          working_tree_dirty: true,
          changed_files: ["src/forecast.py"],
        },
        freshness: { status: "fresh" },
        device_origin: "MacBook Air",
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
        relay_url: "https://relay.iroh.network/",
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
    mockState.status.remote_node_id = parsed.id ?? "iroh1paired...peer";
    mockState.status.connected = true;
    mockState.status.transport_path = "direct_lan";
    mockState.status.rtt_ms = 3;
    return structuredClone(mockState.status);
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
