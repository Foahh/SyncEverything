import { useEffect, useMemo, useState } from "react";
import type { ApprovalDecision, GraphNode, TaskRecord_Serialize } from "./bindings";
import { DagCanvas } from "./components/DagCanvas";
import { InspectorDrawer } from "./components/InspectorDrawer";
import { LeftRail } from "./components/LeftRail";
import { TopTelemetryBar } from "./components/TopTelemetryBar";
import {
  DispatchTaskModal,
  type DispatchTaskFormValues,
} from "./components/modals/DispatchTaskModal";
import { RecordDecisionModal } from "./components/modals/RecordDecisionModal";
import {
  useDagSnapshot,
  useHandoffs,
  useRelayEventSync,
  useResetPreviewDemo,
  useResolvePatchApproval,
  useRuntimeStatus,
  useSimulateArtifactUpdate,
  useSubmitTask,
  useSwitchRuntimeMode,
  useTasks,
  useUpsertDagNode,
} from "./lib/queries";

const THEME_STORAGE_KEY = "relay.theme";

function detectInitialTheme(): "light" | "dark" {
  if (typeof window === "undefined") return "light";
  const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
  if (saved === "light" || saved === "dark") return saved;
  return "light"; // Light-first for crisp classroom projector readability
}

export default function App() {
  useRelayEventSync();

  const [theme, setTheme] = useState<"light" | "dark">(detectInitialTheme);
  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(true);
  const [drawerTab, setDrawerTab] = useState<"node" | "approval">("approval");
  const [activeApprovalTaskId, setActiveApprovalTaskId] = useState<string | null>(null);
  const [decisionModalOpen, setDecisionModalOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);

  const { data: status } = useRuntimeStatus();
  const { data: dag } = useDagSnapshot(includeStale);
  const { data: tasks = [] } = useTasks();
  const { data: handoffs = [] } = useHandoffs();

  const switchMode = useSwitchRuntimeMode();
  const submitTask = useSubmitTask();
  const resolveApproval = useResolvePatchApproval();
  const simulateEdit = useSimulateArtifactUpdate();
  const upsertNode = useUpsertDagNode();
  const resetDemo = useResetPreviewDemo();

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", theme === "dark");
    root.dataset.theme = theme;
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  }, [theme]);

  const selectedNode: GraphNode | null = useMemo(() => {
    if (!selectedNodeId || !dag) return null;
    return dag.nodes.find((n) => n.id === selectedNodeId) ?? null;
  }, [dag, selectedNodeId]);

  const pendingApprovalTask: TaskRecord_Serialize | null = useMemo(() => {
    if (activeApprovalTaskId) {
      const explicit = tasks.find((t) => t.spec.task_id === activeApprovalTaskId);
      if (explicit?.state === "self_healing_proposed" && explicit.active_proposal) {
        return explicit;
      }
    }
    return (
      tasks.find((t) => t.state === "self_healing_proposed" && t.active_proposal != null) ?? null
    );
  }, [tasks, activeApprovalTaskId]);

  const latestArtifact = useMemo(() => {
    if (!dag) return null;
    return [...dag.nodes].reverse().find((n) => n.payload.kind === "artifact") ?? null;
  }, [dag]);

  const handleSelectNode = (nodeId: string | null) => {
    setSelectedNodeId(nodeId);
    if (nodeId) {
      setDrawerTab("node");
      setDrawerOpen(true);
    }
  };

  const handleOpenApproval = (task: TaskRecord_Serialize) => {
    setActiveApprovalTaskId(task.spec.task_id);
    setDrawerTab("approval");
    setDrawerOpen(true);
  };

  const handleTriggerUpstreamEdit = () => {
    if (!latestArtifact) return;
    const randomHex = Math.floor(Math.random() * 0xffffff)
      .toString(16)
      .padStart(6, "0");
    simulateEdit.mutate({
      oldArtifactId: latestArtifact.id,
      newCommitSha: `c4${randomHex}91a0ef`,
      reason: "Modified seasonal weighting in src/forecast.py on laptop",
    });
  };

  const handleRecordDecision = (values: {
    label: string;
    rationale: string;
    constraints: string[];
  }) => {
    const now = new Date().toISOString();
    const id = `dec-${Date.now().toString(36)}`;
    const node: GraphNode = {
      id,
      label: values.label,
      payload: {
        kind: "decision",
        rationale: values.rationale,
        constraints: values.constraints,
      },
      freshness: { status: "fresh" },
      device_origin: status?.mode === "host" ? "win-rtx5060-host" : "macbook-air-m4",
      created_at: now,
      updated_at: now,
    };
    const edges = latestArtifact
      ? [
          {
            id: `informs:${id}->${latestArtifact.id}`,
            from: id,
            to: latestArtifact.id,
            kind: "informs" as const,
          },
        ]
      : [];
    upsertNode.mutate(
      { node, edges },
      {
        onSuccess: () => {
          setSelectedNodeId(id);
          setDrawerTab("node");
          setDrawerOpen(true);
        },
      },
    );
  };

  const handleDispatchTask = (values: DispatchTaskFormValues) => {
    submitTask.mutate({
      task_id: crypto.randomUUID(),
      repo_id: dag?.repo_id ?? "forecast-bench",
      base_commit_sha: dag?.head_commit_sha ?? "HEAD",
      command_preset: values.preset,
      params:
        values.preset === "train_grid" ? { windows: values.windows, alpha: values.alpha } : {},
      require_approval: values.requireApproval,
      requested_by_device: status?.mode === "host" ? "win-rtx5060-host" : "macbook-air-m4",
      user_note: values.userNote.trim() || null,
    });
  };

  const handleResolveApproval = (taskId: string, decision: ApprovalDecision) => {
    resolveApproval.mutate(
      { taskId, decision },
      {
        onSuccess: (updated) => {
          if (updated.evidence_node_id) {
            setSelectedNodeId(updated.evidence_node_id);
            setDrawerTab("node");
          }
        },
      },
    );
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-[var(--relay-canvas)] text-[var(--relay-text)]">
      <TopTelemetryBar
        status={status}
        repoId={dag?.repo_id ?? "forecast-bench"}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        theme={theme}
        onToggleTheme={() => setTheme((prev) => (prev === "light" ? "dark" : "light"))}
        onSwitchMode={(mode) => switchMode.mutate(mode)}
      />

      <main className="flex flex-1 min-h-0 overflow-hidden">
        <LeftRail
          status={status}
          tasks={tasks}
          handoffs={handoffs}
          onOpenApproval={handleOpenApproval}
          onSelectEvidenceNode={handleSelectNode}
          onTriggerUpstreamEdit={handleTriggerUpstreamEdit}
          onResetDemo={() => resetDemo.mutate()}
          isSimulatingEdit={simulateEdit.isPending}
        />

        <DagCanvas
          snapshot={dag}
          selectedNodeId={selectedNodeId}
          includeStale={includeStale}
          onToggleIncludeStale={() => setIncludeStale((prev) => !prev)}
          onSelectNode={handleSelectNode}
          onOpenRecordDecision={() => setDecisionModalOpen(true)}
          onOpenDispatchTask={() => setTaskModalOpen(true)}
          onTriggerUpstreamEdit={handleTriggerUpstreamEdit}
          isSimulatingEdit={simulateEdit.isPending}
        />

        <InspectorDrawer
          open={drawerOpen}
          activeTab={drawerTab}
          onTabChange={setDrawerTab}
          selectedNode={selectedNode}
          pendingApprovalTask={pendingApprovalTask}
          onResolveApproval={handleResolveApproval}
          isResolvingApproval={resolveApproval.isPending}
          onClose={() => setDrawerOpen(false)}
        />
      </main>

      <RecordDecisionModal
        isOpen={decisionModalOpen}
        onOpenChange={setDecisionModalOpen}
        onSubmitDecision={handleRecordDecision}
        isSubmitting={upsertNode.isPending}
      />

      <DispatchTaskModal
        isOpen={taskModalOpen}
        onOpenChange={setTaskModalOpen}
        headCommitSha={dag?.head_commit_sha ?? "HEAD"}
        onDispatch={handleDispatchTask}
        isSubmitting={submitTask.isPending}
      />
    </div>
  );
}
