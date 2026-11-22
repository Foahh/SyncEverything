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
import { PairDeviceModal } from "./components/modals/PairDeviceModal";
import { RecordDecisionModal } from "./components/modals/RecordDecisionModal";
import {
  useDagSnapshot,
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
type ThemeScheme = "light" | "dark";

function readStoredTheme(): ThemeScheme | null {
  if (typeof window === "undefined") return null;
  try {
    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    return saved === "light" || saved === "dark" ? saved : null;
  } catch {
    return null;
  }
}

function readSystemTheme(): ThemeScheme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export default function App() {
  useRelayEventSync();

  const [storedTheme, setStoredTheme] = useState<ThemeScheme | null>(readStoredTheme);
  const [systemTheme, setSystemTheme] = useState<ThemeScheme>(readSystemTheme);
  const theme: ThemeScheme = storedTheme ?? systemTheme;

  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [drawerMode, setDrawerMode] = useState<"closed" | "node" | "fix">("closed");
  const [activeApprovalTaskId, setActiveApprovalTaskId] = useState<string | null>(null);
  const [decisionModalOpen, setDecisionModalOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [pairModalOpen, setPairModalOpen] = useState(false);

  const { data: status } = useRuntimeStatus();
  const { data: dag } = useDagSnapshot(includeStale);
  const { data: tasks = [] } = useTasks();

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
    const meta = document.querySelector<HTMLMetaElement>('meta[name="color-scheme"]');
    if (meta) {
      meta.content = storedTheme ?? "light dark";
    }
  }, [theme, storedTheme]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    const media = window.matchMedia?.("(prefers-color-scheme: dark)");
    const handleMediaChange = (event: MediaQueryListEvent) => {
      setSystemTheme(event.matches ? "dark" : "light");
    };
    const handleStorage = (event: StorageEvent) => {
      if (event.key === THEME_STORAGE_KEY) {
        setStoredTheme(readStoredTheme());
      }
    };
    media?.addEventListener("change", handleMediaChange);
    window.addEventListener("storage", handleStorage);
    return () => {
      media?.removeEventListener("change", handleMediaChange);
      window.removeEventListener("storage", handleStorage);
    };
  }, []);

  const handleToggleTheme = () => {
    const target: ThemeScheme = theme === "light" ? "dark" : "light";
    const currentSystem = readSystemTheme();
    if (target === currentSystem) {
      setStoredTheme(null);
      try {
        window.localStorage.removeItem(THEME_STORAGE_KEY);
      } catch {
        // Ignore storage errors in restricted environments
      }
    } else {
      setStoredTheme(target);
      try {
        window.localStorage.setItem(THEME_STORAGE_KEY, target);
      } catch {
        // Ignore storage errors in restricted environments
      }
    }
  };

  const selectedNode: GraphNode | null = useMemo(() => {
    if (!selectedNodeId || !dag) return null;
    return dag.nodes.find((n) => n.id === selectedNodeId) ?? null;
  }, [dag, selectedNodeId]);

  const pendingFixTask: TaskRecord_Serialize | null = useMemo(() => {
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
      setDrawerMode("node");
    } else if (drawerMode === "node") {
      setDrawerMode("closed");
    }
  };

  const handleOpenApproval = (task: TaskRecord_Serialize) => {
    setActiveApprovalTaskId(task.spec.task_id);
    setDrawerMode("fix");
  };

  const handleTriggerUpstreamEdit = () => {
    if (!latestArtifact) return;
    const randomHex = Math.floor(Math.random() * 0xffffff)
      .toString(16)
      .padStart(6, "0");
    simulateEdit.mutate({
      oldArtifactId: latestArtifact.id,
      newCommitSha: `c4${randomHex}91a0ef`,
      reason: "Updated src/forecast.py",
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
      device_origin: status?.mode === "host" ? "Workstation" : "Laptop",
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
          setDrawerMode("node");
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
      requested_by_device: status?.mode === "host" ? "Workstation" : "Laptop",
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
            setDrawerMode("node");
          } else {
            setDrawerMode("closed");
          }
        },
      },
    );
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <TopTelemetryBar
        status={status}
        repoId={dag?.repo_id ?? "forecast-bench"}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        theme={theme}
        onToggleTheme={handleToggleTheme}
        onOpenPairModal={() => setPairModalOpen(true)}
        onOpenRunTask={() => setTaskModalOpen(true)}
      />

      <main className="flex min-h-0 flex-1 overflow-hidden">
        <LeftRail
          status={status}
          tasks={tasks}
          onOpenRunTask={() => setTaskModalOpen(true)}
          onOpenPairModal={() => setPairModalOpen(true)}
          onOpenApproval={handleOpenApproval}
          onSelectEvidenceNode={handleSelectNode}
          onSwitchMode={(mode) => switchMode.mutate(mode)}
          onTriggerUpstreamEdit={handleTriggerUpstreamEdit}
          onResetDemo={() => {
            setDrawerMode("closed");
            setSelectedNodeId(null);
            resetDemo.mutate();
          }}
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
        />

        <InspectorDrawer
          mode={drawerMode}
          selectedNode={selectedNode}
          pendingFixTask={pendingFixTask}
          onResolveApproval={handleResolveApproval}
          isResolvingApproval={resolveApproval.isPending}
          onClose={() => {
            setDrawerMode("closed");
            setSelectedNodeId(null);
          }}
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

      <PairDeviceModal isOpen={pairModalOpen} onOpenChange={setPairModalOpen} />
    </div>
  );
}
