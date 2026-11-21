import {
  commands,
  events,
  type ApprovalDecision,
  type DagSnapshot_Serialize,
  type DagStaleAlertEvent,
  type GraphEdge,
  type GraphNode,
  type HandoffSummary,
  type PeerStatus_Serialize,
  type RelayStateChangedEvent,
  type RuntimeMode,
  type StalePropagationReport,
  type TaskRecord_Serialize,
  type TaskSpec_Deserialize,
} from "../bindings";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ============================================================================
// Stateful Browser Preview Simulation (matches bindings.ts 1:1 when running
// `pnpm dev` outside a Tauri native window for rapid visual design & review)
// ============================================================================

interface MockWorkspaceState {
  status: PeerStatus_Serialize;
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
      label: "Hybrid MA + EWMA Rolling Forecast",
      payload: {
        kind: "decision",
        rationale:
          "Combine moving average stability with EWMA responsiveness across 7/14/28 rolling horizons; defer heavy grid sweeps to Windows Host over iroh P2P.",
        constraints: ["smoke < 2s", "offline uv.lock", "block STALE citations"],
      },
      freshness: { status: "fresh" },
      device_origin: "macbook-air-m4",
      created_at: t0,
      updated_at: t0,
    },
    {
      id: "art-a91f0c2",
      label: "Commit a91f0c2",
      payload: {
        kind: "artifact",
        git_commit_sha: "a91f0c28e4b71a2d9012f",
        env_fingerprint: "uv:blake3:9c8f4a21",
        working_tree_dirty: false,
        changed_files: ["src/forecast.py", "relay.toml", "pyproject.toml"],
      },
      freshness: { status: "fresh" },
      device_origin: "macbook-air-m4",
      created_at: t1,
      updated_at: t1,
    },
    {
      id: "ev-grid-7-14-28",
      label: "train_grid · Windows 7 / 14 / 28",
      payload: {
        kind: "evidence",
        task_id: "019a9e80-41f2-7d3a-9c44-81b2f3001001",
        preset: "train_grid",
        metrics_json: {
          best_window: 28,
          best_mae: 2.16502,
          rmse_w28: 2.56794,
          alpha: 0.35,
          elapsed_ms: 1.08,
        },
        log_blob_hash: "blake3:7a31d908f42b1c90e442",
      },
      freshness: { status: "fresh" },
      device_origin: "win-rtx5060-host",
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
        requested_by_device: "macbook-air-m4",
        user_note: "Re-run 3-window sweep with seasonal weighting on Host GPU.",
      },
      state: "self_healing_proposed",
      active_proposal: {
        patch_id: "patch-uv-scipy-01",
        task_id: "019a9e80-78c1-7a91-b302-9a10e44b2002",
        kind: "uv_spec",
        summary:
          "Detected ModuleNotFoundError('scipy') during isolated smoke preflight. Added pinned scipy>=1.15 to pyproject.toml and regenerated uv.lock inside worktree.",
        diff_preview: [
          "diff --git a/pyproject.toml b/pyproject.toml",
          "index 4f21a09..98b2c44 100644",
          "--- a/pyproject.toml",
          "+++ b/pyproject.toml",
          '@@ -4,7 +4,9 @@ version = "0.1.0"',
          ' description = "Reproducible time-series window forecasting fixture"',
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
          '[uv] Resolved 2 packages in 18ms (offline cache hit)\n[smoke] {"mode":"smoke","best_window":7,"best_mae":2.46437,"elapsed_ms":0.24}\n[preflight] Exit code 0 · Main worktree untouched',
        created_at: t3,
      },
      preflight_summary:
        "Missing dependency healed in isolated worktree; awaiting Elicitation approval.",
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
        requested_by_device: "macbook-air-m4",
        user_note: "Baseline 3-window sweep before seasonal weighting refactor.",
      },
      state: "completed",
      active_proposal: null,
      preflight_summary: "Smoke passed in 0.21ms",
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
      source_device: "macbook-air-m4",
      base_commit_sha: "a91f0c2",
      summary: "Queued seasonal weighting grid sweep after offline laptop edits",
      active_task_count: 1,
      stale_evidence_count: 0,
      created_at: t3,
    },
    {
      handoff_id: "ho-20261121-01",
      repo_id: "forecast-bench",
      source_device: "win-rtx5060-host",
      base_commit_sha: "a91f0c2",
      summary: "Returned baseline MAE 2.16502 (best window=28) with BLAKE3 log proof",
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

  async switchRuntimeMode(mode: RuntimeMode): Promise<RuntimeMode> {
    if (isTauriRuntime()) {
      return commands.switchRuntimeMode(mode);
    }
    mockState.status.mode = mode;
    mockState.status.transport_path =
      mode === "local_dual_sandbox" ? "loopback" : mode === "client" ? "relay" : "direct_lan";
    mockState.status.rtt_ms = mode === "local_dual_sandbox" ? 1 : mode === "client" ? 19 : 2;
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
      preflight_summary: "Queued for isolated git worktree smoke check",
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
      task.preflight_summary = "Patch merged cleanly in worktree and experiment completed.";
      const evidenceId = `ev-${taskId.slice(0, 8)}`;
      task.evidence_node_id = evidenceId;
      const latestArtifact = [...mockState.dag.nodes]
        .reverse()
        .find((n) => n.payload.kind === "artifact");
      if (!mockState.dag.nodes.some((n) => n.id === evidenceId)) {
        mockState.dag.nodes.push({
          id: evidenceId,
          label: `${task.spec.command_preset} · Healed Run`,
          payload: {
            kind: "evidence",
            task_id: task.spec.task_id,
            preset: task.spec.command_preset,
            metrics_json: {
              best_window: 28,
              best_mae: 1.94218,
              rmse_w28: 2.31044,
              alpha: Number(task.spec.params.alpha ?? 0.45),
              elapsed_ms: 1.14,
            },
            log_blob_hash: "blake3:9e12ca44110b83df2210",
          },
          freshness: { status: "fresh" },
          device_origin: "win-rtx5060-host",
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
      task.preflight_summary = "Patch proposal rejected by operator; worktree discarded.";
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
          env_fingerprint: "uv:blake3:9c8f4a21",
          working_tree_dirty: true,
          changed_files: ["src/forecast.py"],
        },
        freshness: { status: "fresh" },
        device_origin: "macbook-air-m4",
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

    // BFS downstream propagation matching relay_core::dag::DagEngine
    const visited = new Set<string>([newId]);
    const queue = [newId];
    const invalidated: string[] = [];

    while (queue.length > 0) {
      const current = queue.shift()!;
      for (const edge of mockState.dag.edges) {
        if (edge.from === current && !visited.has(edge.to)) {
          visited.add(edge.to);
          queue.push(edge.to);
          const target = mockState.dag.nodes.find((n) => n.id === edge.to);
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
