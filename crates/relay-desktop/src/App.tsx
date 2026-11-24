import { useMemo, useState } from "react";
import type { GraphNode, TaskRecord_Serialize } from "./bindings";
import { findCommitArtifact, needsTaskApproval, type ApprovalInput } from "./lib/relayApi";
import { DagCanvas } from "./components/DagCanvas";
import { LeftRail } from "./components/LeftRail";
import { RightWorkbenchRail, type RightRailTab } from "./components/RightWorkbenchRail";
import { TopTelemetryBar } from "./components/TopTelemetryBar";
import {
  DispatchTaskModal,
  type DispatchTaskFormValues,
} from "./components/modals/DispatchTaskModal";
import { PairDeviceModal } from "./components/modals/PairDeviceModal";
import { RecordDecisionModal } from "./components/modals/RecordDecisionModal";
import {
  WorkspaceSettingsModal,
  type SettingsTab,
} from "./components/modals/WorkspaceSettingsModal";
import { useCopilotChat } from "./hooks/useCopilotChat";
import { useRelayTheme } from "./hooks/useRelayTheme";
import {
  useDagSnapshot,
  useLlmSettings,
  useRelayEventSync,
  useResetPreviewDemo,
  useResolvePatchApproval,
  useRuntimeStatus,
  useSimulateArtifactUpdate,
  useSubmitTask,
  useSwitchRuntimeMode,
  useTasks,
  useUpsertDagNode,
  useWorkspaceInfo,
} from "./lib/queries";

export default function App() {
  useRelayEventSync();
  const { theme, toggleTheme } = useRelayTheme();

  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [rightRailOpen, setRightRailOpen] = useState(true);
  const [rightRailTab, setRightRailTab] = useState<RightRailTab>("agent");
  const [inspectorMode, setInspectorMode] = useState<"node" | "fix">("node");
  const [activeApprovalTaskId, setActiveApprovalTaskId] = useState<string | null>(null);
  const [decisionModalOpen, setDecisionModalOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [pairModalOpen, setPairModalOpen] = useState(false);
  const [settingsModalOpen, setSettingsModalOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("repo");

  const { data: status } = useRuntimeStatus();
  const { data: workspace } = useWorkspaceInfo();
  const { data: llmSettings } = useLlmSettings();
  const { data: dag } = useDagSnapshot(includeStale);
  const { data: tasks = [] } = useTasks();
  const copilot = useCopilotChat();

  const switchMode = useSwitchRuntimeMode();
  const submitTask = useSubmitTask();
  const resolveApproval = useResolvePatchApproval();
  const simulateEdit = useSimulateArtifactUpdate();
  const upsertNode = useUpsertDagNode();
  const resetDemo = useResetPreviewDemo();

  const selectedNode: GraphNode | null = useMemo(() => {
    if (!selectedNodeId || !dag) return null;
    return dag.nodes.find((n) => n.id === selectedNodeId) ?? null;
  }, [dag, selectedNodeId]);

  const pendingFixTask: TaskRecord_Serialize | null = useMemo(() => {
    if (activeApprovalTaskId) {
      const explicit = tasks.find((t) => t.spec.task_id === activeApprovalTaskId);
      return explicit && needsTaskApproval(explicit) ? explicit : null;
    }
    return tasks.find(needsTaskApproval) ?? null;
  }, [tasks, activeApprovalTaskId]);

  const latestArtifact = useMemo(() => {
    if (!dag) return null;
    return findCommitArtifact(dag, dag.head_commit_sha);
  }, [dag]);

  const handleSelectNode = (nodeId: string | null) => {
    setSelectedNodeId(nodeId);
    if (nodeId) {
      setInspectorMode("node");
      setRightRailTab("inspector");
      setRightRailOpen(true);
    }
  };

  const handleOpenApproval = (task: TaskRecord_Serialize) => {
    setActiveApprovalTaskId(task.spec.task_id);
    setInspectorMode("fix");
    setRightRailTab("inspector");
    setRightRailOpen(true);
  };

  const handleToggleCopilot = () => {
    if (rightRailOpen && rightRailTab === "agent") {
      setRightRailOpen(false);
    } else {
      setRightRailTab("agent");
      setRightRailOpen(true);
    }
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

  const handleRecordDecision = async (values: {
    label: string;
    rationale: string;
    constraints: string[];
  }) => {
    const now = new Date().toISOString();
    const id = `dec-${crypto.randomUUID()}`;
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
    await upsertNode.mutateAsync({ node, edges });
    setSelectedNodeId(id);
    setInspectorMode("node");
    setRightRailTab("inspector");
    setRightRailOpen(true);
  };

  const handleDispatchTask = async (values: DispatchTaskFormValues) => {
    await submitTask.mutateAsync({
      task_id: values.taskId,
      repo_id: workspace?.repo_id ?? dag?.repo_id ?? "forecast-bench",
      base_commit_sha: dag?.head_commit_sha ?? "HEAD",
      command_preset: values.preset,
      params: values.params,
      require_approval: values.requireApproval,
      requested_by_device: status?.mode === "host" ? "Workstation" : "Laptop",
      user_note: values.userNote.trim() || null,
    });
  };

  const handleOpenSettings = (tab: SettingsTab) => {
    setSettingsTab(tab);
    setSettingsModalOpen(true);
  };

  const handleResolveApproval = (approval: ApprovalInput, fromTab: RightRailTab) => {
    resolveApproval.mutate(approval, {
      onSuccess: (updated) => {
        if (fromTab === "agent") {
          return;
        }
        if (updated.evidence_node_id) {
          setSelectedNodeId(updated.evidence_node_id);
          setInspectorMode("node");
          setRightRailTab("inspector");
        } else {
          setRightRailTab("agent");
        }
      },
    });
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <TopTelemetryBar
        status={status}
        repoId={workspace?.repo_id ?? dag?.repo_id ?? "forecast-bench"}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        theme={theme}
        isCopilotOpen={rightRailOpen && rightRailTab === "agent"}
        onToggleTheme={toggleTheme}
        onToggleCopilot={handleToggleCopilot}
        onOpenPairModal={() => setPairModalOpen(true)}
        onOpenRunTask={() => setTaskModalOpen(true)}
        onOpenRepoSettings={() => handleOpenSettings("repo")}
        onOpenLlmSettings={() => handleOpenSettings("llm")}
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
            setSelectedNodeId(null);
            setInspectorMode("node");
            setRightRailTab("agent");
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

        <RightWorkbenchRail
          isOpen={rightRailOpen}
          activeTab={rightRailTab}
          onTabChange={setRightRailTab}
          onClose={() => {
            setRightRailOpen(false);
            setSelectedNodeId(null);
          }}
          inspectorMode={inspectorMode}
          selectedNode={selectedNode}
          pendingFixTask={pendingFixTask}
          onResolveApproval={handleResolveApproval}
          isResolvingApproval={resolveApproval.isPending}
          approvalError={resolveApproval.error == null ? null : String(resolveApproval.error)}
          messages={copilot.messages}
          isStreaming={copilot.isStreaming}
          streamingMessageId={copilot.streamingMessageId}
          sendError={copilot.sendError}
          llmSettings={llmSettings}
          onSendMessage={copilot.sendMessage}
          onCancelTurn={copilot.cancelTurn}
          onClearChat={copilot.clearChat}
          isClearingChat={copilot.isClearing}
          onOpenLlmSettings={() => handleOpenSettings("llm")}
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
        presets={workspace?.presets ?? []}
        onDispatch={handleDispatchTask}
        isSubmitting={submitTask.isPending}
      />

      <PairDeviceModal isOpen={pairModalOpen} onOpenChange={setPairModalOpen} />

      <WorkspaceSettingsModal
        isOpen={settingsModalOpen}
        onOpenChange={setSettingsModalOpen}
        activeTab={settingsTab}
        onTabChange={setSettingsTab}
      />
    </div>
  );
}
