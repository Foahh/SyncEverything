import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { GraphNode, TaskRecord_Serialize } from "./bindings";
import { formatErrorMessage } from "./lib/formatError";
import { findCommitArtifact, needsTaskApproval, type ApprovalInput } from "./lib/relayApi";
import { DagCanvas } from "./components/DagCanvas";
import type { InspectorMode } from "./components/InspectorDrawer";
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
import { useTaskLogStream } from "./hooks/useTaskLogStream";
import {
  useCancelTask,
  useDagSnapshot,
  useInitWorkspaceManifest,
  useLlmSettings,
  useRecentWorkspaces,
  useRelayEventSync,
  useRemoveRecentWorkspace,
  useResetPreviewDemo,
  useResolvePatchApproval,
  useRuntimeStatus,
  useSimulateArtifactUpdate,
  useSubmitTask,
  useSwitchRuntimeMode,
  useSwitchWorkspace,
  useTaskLog,
  useTasks,
  useTrustedPeers,
  useUpsertDagNode,
  useWorkspaceInfo,
} from "./lib/queries";
import { isTaskRunning } from "./lib/taskLogStream";

export default function App() {
  useRelayEventSync();
  const { t } = useTranslation();
  const { theme, toggleTheme } = useRelayTheme();

  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [rightRailOpen, setRightRailOpen] = useState(true);
  const [rightRailTab, setRightRailTab] = useState<RightRailTab>("agent");
  const [inspectorMode, setInspectorMode] = useState<InspectorMode>("node");
  const [activeApprovalTaskId, setActiveApprovalTaskId] = useState<string | null>(null);
  const [decisionModalOpen, setDecisionModalOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [pairModalOpen, setPairModalOpen] = useState(false);
  const [settingsModalOpen, setSettingsModalOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("repo");
  const [settingsNotice, setSettingsNotice] = useState<string | null>(null);

  const { data: status } = useRuntimeStatus();
  const { data: workspace } = useWorkspaceInfo();
  const { data: recentWorkspaces = [] } = useRecentWorkspaces();
  const { data: llmSettings } = useLlmSettings();
  const { data: dag } = useDagSnapshot(includeStale);
  const { data: tasks = [] } = useTasks();
  const { data: trustedPeers = [] } = useTrustedPeers();
  const copilot = useCopilotChat();
  const { logsByTask, clearTaskLogs } = useTaskLogStream();

  const switchMode = useSwitchRuntimeMode();
  const switchWorkspace = useSwitchWorkspace();
  const removeRecentWorkspace = useRemoveRecentWorkspace();
  const submitTask = useSubmitTask();
  const cancelTask = useCancelTask();
  const resolveApproval = useResolvePatchApproval();
  const simulateEdit = useSimulateArtifactUpdate();
  const upsertNode = useUpsertDagNode();
  const resetDemo = useResetPreviewDemo();
  const initManifest = useInitWorkspaceManifest();

  const selectedNode: GraphNode | null = useMemo(() => {
    if (!selectedNodeId || !dag) return null;
    return dag.nodes.find((n) => n.id === selectedNodeId) ?? null;
  }, [dag, selectedNodeId]);

  const selectedTask: TaskRecord_Serialize | null = useMemo(() => {
    if (!selectedTaskId) return null;
    return tasks.find((t) => t.spec.task_id === selectedTaskId) ?? null;
  }, [tasks, selectedTaskId]);

  const { data: persistedTaskLog } = useTaskLog(
    selectedTaskId,
    inspectorMode === "task" && selectedTask != null && !isTaskRunning(selectedTask.state),
  );

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
    setSelectedTaskId(task.spec.task_id);
    setInspectorMode("fix");
    setRightRailTab("inspector");
    setRightRailOpen(true);
  };

  const handleInspectTask = (task: TaskRecord_Serialize) => {
    setSelectedTaskId(task.spec.task_id);
    setInspectorMode("task");
    setRightRailTab("inspector");
    setRightRailOpen(true);
  };

  const handleCancelTask = (taskId: string) => {
    cancelTask.mutate(taskId);
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
      reason: "Simulated upstream code change",
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
    const created = await submitTask.mutateAsync({
      task_id: values.taskId,
      repo_id: workspace?.repo_id ?? dag?.repo_id ?? "default-workspace",
      base_commit_sha: dag?.head_commit_sha ?? "HEAD",
      command_preset: values.preset,
      params: values.params,
      require_approval: values.requireApproval,
      requested_by_device: status?.mode === "host" ? "Workstation" : "Laptop",
      user_note: values.userNote.trim() || null,
    });
    setSelectedTaskId(created.spec.task_id);
    setInspectorMode("task");
    setRightRailTab("inspector");
    setRightRailOpen(true);
  };

  const handleOpenSettings = (tab: SettingsTab) => {
    setSettingsNotice(null);
    setSettingsTab(tab);
    setSettingsModalOpen(true);
  };

  const openPairing = () => {
    if (workspace?.repo_root) {
      setPairModalOpen(true);
    } else {
      setSettingsNotice(t("modals.workspace.pairRequiresRepoHint"));
      setSettingsTab("repo");
      setSettingsModalOpen(true);
    }
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

  const handleSwitchWorkspace = async (repoPath: string) => {
    await switchWorkspace.mutateAsync(repoPath);
    setSelectedNodeId(null);
    setSelectedTaskId(null);
    clearTaskLogs();
    setActiveApprovalTaskId(null);
    setInspectorMode("node");
  };

  const handleRemoveRecentWorkspace = async (repoPath: string) => {
    await removeRecentWorkspace.mutateAsync(repoPath);
  };

  const handleInitWorkspaceManifest = async () => {
    await initManifest.mutateAsync({ repoPath: workspace?.repo_root ?? null });
  };

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <TopTelemetryBar
        status={status}
        workspace={workspace}
        recentWorkspaces={recentWorkspaces}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        theme={theme}
        isCopilotOpen={rightRailOpen && rightRailTab === "agent"}
        isSwitchingWorkspace={switchWorkspace.isPending}
        isInitializingManifest={initManifest.isPending}
        onToggleTheme={toggleTheme}
        onToggleCopilot={handleToggleCopilot}
        onOpenPairModal={openPairing}
        onOpenRunTask={() => setTaskModalOpen(true)}
        onOpenRepoSettings={() => handleOpenSettings("repo")}
        onOpenLlmSettings={() => handleOpenSettings("llm")}
        onSwitchWorkspace={handleSwitchWorkspace}
        onRemoveRecentWorkspace={handleRemoveRecentWorkspace}
        onInitManifest={handleInitWorkspaceManifest}
      />

      <main className="flex min-h-0 flex-1 overflow-hidden">
        <LeftRail
          status={status}
          trustedPeers={trustedPeers}
          tasks={tasks}
          selectedTaskId={inspectorMode === "task" ? selectedTaskId : null}
          cancellingTaskId={cancelTask.isPending ? (cancelTask.variables ?? null) : null}
          onOpenRunTask={() => setTaskModalOpen(true)}
          onOpenPairModal={openPairing}
          onOpenApproval={handleOpenApproval}
          onInspectTask={handleInspectTask}
          onCancelTask={handleCancelTask}
          onSelectEvidenceNode={handleSelectNode}
          onSwitchMode={(mode) => switchMode.mutate(mode)}
          onTriggerUpstreamEdit={handleTriggerUpstreamEdit}
          onResetDemo={() => {
            setSelectedNodeId(null);
            setSelectedTaskId(null);
            clearTaskLogs();
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
          hasRepo={Boolean(workspace?.repo_root)}
          isGitRepo={Boolean(workspace?.is_git_repo)}
          hasManifest={Boolean(workspace?.has_manifest)}
          isGeneratingManifest={initManifest.isPending}
          onToggleIncludeStale={() => setIncludeStale((prev) => !prev)}
          onSelectNode={handleSelectNode}
          onOpenRecordDecision={() => setDecisionModalOpen(true)}
          onOpenDispatchTask={() => setTaskModalOpen(true)}
          onOpenRepoSettings={() => handleOpenSettings("repo")}
          manifestError={initManifest.error ? formatErrorMessage(initManifest.error, t) : null}
          onDismissManifestError={() => initManifest.reset()}
          onGenerateManifest={() => initManifest.mutate({ repoPath: workspace?.repo_root ?? null })}
        />

        <RightWorkbenchRail
          isOpen={rightRailOpen}
          activeTab={rightRailTab}
          onTabChange={setRightRailTab}
          onClose={() => {
            setRightRailOpen(false);
            setSelectedNodeId(null);
            setSelectedTaskId(null);
          }}
          inspectorMode={inspectorMode}
          selectedNode={selectedNode}
          selectedTask={selectedTask}
          taskLogBuffer={selectedTask ? logsByTask[selectedTask.spec.task_id] : undefined}
          persistedTaskLog={persistedTaskLog}
          pendingFixTask={pendingFixTask}
          onResolveApproval={handleResolveApproval}
          isResolvingApproval={resolveApproval.isPending}
          approvalError={
            resolveApproval.error == null ? null : formatErrorMessage(resolveApproval.error, t)
          }
          onDismissApprovalError={() => resolveApproval.reset()}
          onCancelTask={handleCancelTask}
          cancellingTaskId={cancelTask.isPending ? (cancelTask.variables ?? null) : null}
          cancelError={cancelTask.error == null ? null : formatErrorMessage(cancelTask.error, t)}
          onDismissCancelError={() => cancelTask.reset()}
          onOpenApproval={handleOpenApproval}
          onSelectEvidenceNode={handleSelectNode}
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
        headCommitSha={dag?.head_commit_sha ?? null}
        presets={workspace?.presets ?? []}
        onDispatch={handleDispatchTask}
        isSubmitting={submitTask.isPending}
        onOpenRepoSettings={() => handleOpenSettings("repo")}
      />

      <PairDeviceModal
        key={`${workspace?.repo_root}:${status?.mode}`}
        mode={status?.mode ?? "local_dual_sandbox"}
        status={status}
        isOpen={pairModalOpen}
        onOpenChange={setPairModalOpen}
      />

      <WorkspaceSettingsModal
        isOpen={settingsModalOpen}
        onOpenChange={(open) => {
          setSettingsModalOpen(open);
          if (!open) setSettingsNotice(null);
        }}
        activeTab={settingsTab}
        onTabChange={setSettingsTab}
        repoNotice={workspace?.repo_root ? null : settingsNotice}
      />
    </div>
  );
}
