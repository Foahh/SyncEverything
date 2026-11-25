import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { GraphNode, TaskRecord_Serialize } from "./bindings";
import { DagCanvas } from "./components/DagCanvas";
import type { InspectorMode } from "./components/InspectorDrawer";
import { RightWorkbenchRail, type RightRailTab } from "./components/RightWorkbenchRail";
import { TaskWorkbench, type TaskFilter } from "./components/TaskWorkbench";
import { TopTelemetryBar, type WorkspaceView } from "./components/TopTelemetryBar";
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
import { formatErrorMessage } from "./lib/formatError";
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
  useUpsertDagNode,
  useWorkspaceInfo,
} from "./lib/queries";
import { findCommitArtifact, needsTaskApproval, type ApprovalInput } from "./lib/relayApi";
import { isTaskRunning } from "./lib/taskLogStream";

const emptyTasks: TaskRecord_Serialize[] = [];

function needsAttention(task: TaskRecord_Serialize): boolean {
  return needsTaskApproval(task) || task.state === "failed";
}

function isActiveTask(task: TaskRecord_Serialize): boolean {
  return task.state === "queued" || isTaskRunning(task.state);
}

export default function App() {
  useRelayEventSync();
  const { t } = useTranslation();
  const { theme, toggleTheme } = useRelayTheme();

  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [rightRailOpen, setRightRailOpen] = useState(true);
  const [activeView, setActiveView] = useState<WorkspaceView>("tasks");
  const [filter, setFilter] = useState<TaskFilter>("all");
  const [dispatchHead, setDispatchHead] = useState<string | null>(null);
  const [seed, setSeed] = useState<TaskRecord_Serialize | null>(null);
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
  const { data: dag } = useDagSnapshot(activeView === "graph" ? includeStale : true);
  const taskQuery = useTasks();
  const tasks = taskQuery.data ?? emptyTasks;
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
    return tasks.find((task) => task.spec.task_id === selectedTaskId) ?? null;
  }, [tasks, selectedTaskId]);

  const { data: persistedTaskLog } = useTaskLog(
    selectedTaskId,
    inspectorMode === "task" &&
      selectedTask != null &&
      (!isTaskRunning(selectedTask.state) || needsTaskApproval(selectedTask)),
  );

  const pendingFixTask: TaskRecord_Serialize | null = useMemo(() => {
    if (activeApprovalTaskId) {
      const explicit = tasks.find((task) => task.spec.task_id === activeApprovalTaskId);
      return explicit && needsTaskApproval(explicit) ? explicit : null;
    }
    return tasks.find(needsTaskApproval) ?? null;
  }, [tasks, activeApprovalTaskId]);

  const latestArtifact = useMemo(() => {
    if (!dag) return null;
    return findCommitArtifact(dag, dag.head_commit_sha);
  }, [dag]);

  const filterCounts = useMemo<Record<TaskFilter, number>>(() => {
    let attentionCount = 0;
    let activeCount = 0;
    let historyCount = 0;
    for (const task of tasks) {
      if (needsAttention(task)) {
        attentionCount += 1;
      } else if (isActiveTask(task)) {
        activeCount += 1;
      } else {
        historyCount += 1;
      }
    }
    return {
      all: tasks.length,
      attention: attentionCount,
      active: activeCount,
      history: historyCount,
    };
  }, [tasks]);

  const visibleTasks = useMemo(
    () =>
      tasks
        .filter(
          (task) =>
            filter === "all" ||
            (filter === "attention"
              ? needsAttention(task)
              : filter === "active"
                ? isActiveTask(task) && !needsAttention(task)
                : !isActiveTask(task) && !needsAttention(task)),
        )
        .sort(
          (a, b) =>
            Number(needsAttention(b)) - Number(needsAttention(a)) ||
            b.updated_at.localeCompare(a.updated_at),
        ),
    [tasks, filter],
  );

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

  const currentDeviceLabel =
    status?.mode === "host"
      ? t("sidebar.mode.host")
      : status?.mode === "local_dual_sandbox"
        ? t("sidebar.mode.local_dual_sandbox")
        : t("sidebar.mode.client");

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
      device_origin: currentDeviceLabel,
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

  function openRun(previous: TaskRecord_Serialize | null = null) {
    if (
      !workspace?.is_git_repo ||
      !workspace.has_manifest ||
      !dag?.head_commit_sha ||
      !workspace.presets.length
    ) {
      handleOpenSettings("repo");
      return;
    }
    if (status?.mode === "client" && !status.remote_node_id && !status.connected) {
      openPairing();
      return;
    }
    setSeed(previous);
    setDispatchHead(dag.head_commit_sha);
    setTaskModalOpen(true);
  }

  const handleDispatchTask = async (values: DispatchTaskFormValues) => {
    if (status?.mode === "client" && !status.connected && !status.remote_node_id) {
      throw new Error(t("topbar.pairDevice"));
    }
    if (!dispatchHead || dispatchHead !== dag?.head_commit_sha) {
      setDispatchHead(dag?.head_commit_sha ?? null);
      throw new Error(t("desk.changed"));
    }
    const created = await submitTask.mutateAsync({
      task_id: values.taskId,
      repo_id: workspace?.repo_id ?? dag?.repo_id ?? "default-workspace",
      base_commit_sha: dispatchHead,
      command_preset: values.preset,
      params: values.params,
      require_approval: values.requireApproval,
      requested_by_device: currentDeviceLabel,
      user_note: values.userNote.trim() || null,
    });
    setSelectedTaskId(created.spec.task_id);
    setInspectorMode("task");
    setRightRailTab("inspector");
    setRightRailOpen(true);
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
          setSelectedTaskId(updated.spec.task_id);
          setInspectorMode("task");
          setRightRailTab("inspector");
        }
      },
    });
  };

  const handleSwitchWorkspace = async (repoPath: string) => {
    await switchWorkspace.mutateAsync(repoPath);
    setActiveView("tasks");
    setFilter("all");
    setTaskModalOpen(false);
    setSeed(null);
    setDispatchHead(null);
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

  const runActionText =
    status?.mode === "client"
      ? status.connected
        ? t("sidebar.runOnHost")
        : t("desk.enqueue")
      : t("desk.local");

  const emptyActionLabel = !workspace?.repo_root
    ? t("desk.open")
    : !workspace.has_manifest || !dag?.head_commit_sha || !workspace.presets.length
      ? t("desk.configure")
      : status?.mode === "client" && !status.connected && !status.remote_node_id
        ? t("topbar.pairDevice")
        : runActionText;

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background text-foreground">
      <TopTelemetryBar
        activeView={activeView}
        onViewChange={setActiveView}
        attentionCount={filterCounts.attention}
        status={status}
        workspace={workspace}
        recentWorkspaces={recentWorkspaces}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        theme={theme}
        isCopilotOpen={rightRailOpen && rightRailTab === "agent"}
        isSwitchingWorkspace={switchWorkspace.isPending}
        isInitializingManifest={initManifest.isPending}
        isSwitchingMode={switchMode.isPending}
        switchModeError={switchMode.error == null ? null : formatErrorMessage(switchMode.error, t)}
        onDismissSwitchModeError={() => switchMode.reset()}
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
        onToggleTheme={toggleTheme}
        onToggleCopilot={handleToggleCopilot}
        onOpenPairModal={openPairing}
        onOpenRunTask={() => openRun()}
        onOpenRepoSettings={() => handleOpenSettings("repo")}
        onOpenLlmSettings={() => handleOpenSettings("llm")}
        onSwitchWorkspace={handleSwitchWorkspace}
        onRemoveRecentWorkspace={handleRemoveRecentWorkspace}
        onInitManifest={handleInitWorkspaceManifest}
      />

      <main className="flex min-h-0 flex-1 overflow-hidden">
        {activeView === "tasks" ? (
          <TaskWorkbench
            status={status}
            dag={dag}
            tasks={visibleTasks}
            filter={filter}
            counts={filterCounts}
            isLoading={taskQuery.isPending}
            errorText={taskQuery.isError ? formatErrorMessage(taskQuery.error, t) : null}
            onRetryLoad={() => void taskQuery.refetch()}
            onFilterChange={setFilter}
            emptyActionLabel={emptyActionLabel}
            onPrimaryAction={() => openRun()}
            onRerun={openRun}
            selectedTaskId={inspectorMode === "task" ? selectedTaskId : null}
            cancellingTaskId={cancelTask.isPending ? (cancelTask.variables ?? null) : null}
            onOpenApproval={handleOpenApproval}
            onInspectTask={handleInspectTask}
            onCancelTask={handleCancelTask}
            onSelectEvidenceNode={handleSelectNode}
          />
        ) : (
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
            onOpenDispatchTask={() => openRun()}
            onOpenRepoSettings={() => handleOpenSettings("repo")}
            manifestError={initManifest.error ? formatErrorMessage(initManifest.error, t) : null}
            onDismissManifestError={() => initManifest.reset()}
            onGenerateManifest={() =>
              initManifest.mutate({ repoPath: workspace?.repo_root ?? null })
            }
          />
        )}

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
          onDismissSendError={copilot.clearSendError}
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
        headCommitSha={dispatchHead}
        seed={seed}
        runText={runActionText}
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
