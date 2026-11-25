import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { GraphNode, TaskRecord_Serialize } from "./bindings";
import { AppSidebar, type AppNavView } from "./components/AppSidebar";
import { DagCanvas } from "./components/DagCanvas";
import { DevicesView } from "./components/DevicesView";
import type { InspectorMode } from "./components/InspectorDrawer";
import { RightWorkbenchRail, type RightRailTab } from "./components/RightWorkbenchRail";
import { SettingsView, type SettingsSection } from "./components/SettingsView";
import { TaskWorkbench, type TaskFilter } from "./components/TaskWorkbench";
import { WorkspaceHeader } from "./components/WorkspaceHeader";
import { WorkspaceWelcome } from "./components/WorkspaceWelcome";
import {
  DispatchTaskModal,
  type DispatchTaskFormValues,
} from "./components/modals/DispatchTaskModal";
import { RecordDecisionModal } from "./components/modals/RecordDecisionModal";
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
  useResolvePatchApproval,
  useRuntimeStatus,
  useSubmitTask,
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

  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [activeView, setActiveView] = useState<AppNavView>("tasks");
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("llm");
  const [includeStale, setIncludeStale] = useState(true);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(null);
  const [rightRailOpen, setRightRailOpen] = useState(false);
  const [filter, setFilter] = useState<TaskFilter>("all");
  const [dispatchHead, setDispatchHead] = useState<string | null>(null);
  const [seed, setSeed] = useState<TaskRecord_Serialize | null>(null);
  const [rightRailTab, setRightRailTab] = useState<RightRailTab>("agent");
  const [inspectorMode, setInspectorMode] = useState<InspectorMode>("node");
  const [activeApprovalTaskId, setActiveApprovalTaskId] = useState<string | null>(null);
  const [decisionModalOpen, setDecisionModalOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);

  const { data: status } = useRuntimeStatus();
  const { data: workspace } = useWorkspaceInfo();
  const { data: recentWorkspaces = [] } = useRecentWorkspaces();
  const { data: llmSettings } = useLlmSettings();
  const { data: dag } = useDagSnapshot(activeView === "graph" ? includeStale : true);
  const taskQuery = useTasks();
  const tasks = taskQuery.data ?? emptyTasks;
  const copilot = useCopilotChat();
  const { logsByTask, clearTaskLogs } = useTaskLogStream();

  const switchWorkspace = useSwitchWorkspace();
  const removeRecentWorkspace = useRemoveRecentWorkspace();
  const submitTask = useSubmitTask();
  const cancelTask = useCancelTask();
  const resolveApproval = useResolvePatchApproval();
  const upsertNode = useUpsertDagNode();
  const initManifest = useInitWorkspaceManifest();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "l") {
        event.preventDefault();
        setRightRailOpen((open) => {
          if (!open) {
            setRightRailTab("agent");
            return true;
          }
          return false;
        });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

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

  const hasRepo = Boolean(workspace?.repo_root);
  const repoName = hasRepo ? workspace?.name || workspace?.repo_id || null : null;

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

  const handleToggleRightRail = () => {
    setRightRailOpen((open) => {
      if (!open) {
        setRightRailTab("agent");
        return true;
      }
      return false;
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

  const openSettingsSection = (section: SettingsSection) => {
    setSettingsSection(section);
    setActiveView("settings");
  };

  function openRun(previous: TaskRecord_Serialize | null = null) {
    if (!workspace?.repo_root) {
      setActiveView("tasks");
      return;
    }
    if (!workspace.is_git_repo || !workspace.has_manifest || !workspace.presets.length) {
      setSeed(null);
      setDispatchHead(dag?.head_commit_sha ?? null);
      setTaskModalOpen(true);
      return;
    }
    if (status?.mode === "client" && !status.remote_node_id && !status.connected) {
      setActiveView("devices");
      return;
    }
    setSeed(previous);
    setDispatchHead(dag?.head_commit_sha ?? null);
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
    if (activeView === "devices" || activeView === "settings") {
      // stay on current utility view if user switched repo from sidebar while there
    } else {
      setActiveView("tasks");
    }
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

  const emptyActionLabel =
    !workspace?.has_manifest || !workspace?.presets.length
      ? t("canvas.generateManifestBtn")
      : status?.mode === "client" && !status.connected && !status.remote_node_id
        ? t("topbar.pairDevice")
        : runActionText;

  const handleTasksEmptyPrimaryAction = () => {
    if (workspace?.is_git_repo && !workspace.has_manifest) {
      void handleInitWorkspaceManifest();
      return;
    }
    openRun();
  };

  const hasRightRailActivity =
    pendingFixTask != null || filterCounts.active > 0 || copilot.isStreaming;

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <AppSidebar
        isOpen={sidebarOpen}
        onCollapse={() => setSidebarOpen(false)}
        activeView={activeView}
        onViewChange={setActiveView}
        attentionCount={filterCounts.attention}
        totalTaskCount={filterCounts.all}
        staleCount={dag?.stale_count ?? 0}
        status={status}
        workspace={workspace}
        recentWorkspaces={recentWorkspaces}
        headCommitSha={dag?.head_commit_sha ?? null}
        theme={theme}
        onToggleTheme={toggleTheme}
        isSwitchingWorkspace={switchWorkspace.isPending}
        isInitializingManifest={initManifest.isPending}
        onSwitchWorkspace={handleSwitchWorkspace}
        onRemoveRecentWorkspace={handleRemoveRecentWorkspace}
        onInitManifest={handleInitWorkspaceManifest}
      />

      <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <WorkspaceHeader
          activeView={activeView}
          hasRepo={hasRepo}
          repoName={repoName}
          headCommitSha={dag?.head_commit_sha ?? null}
          staleCount={dag?.stale_count ?? 0}
          sidebarOpen={sidebarOpen}
          onExpandSidebar={() => setSidebarOpen(true)}
          rightRailOpen={rightRailOpen}
          hasRightRailActivity={hasRightRailActivity}
          onToggleRightRail={handleToggleRightRail}
          onOpenRunTask={() => openRun()}
          onOpenRecordDecision={() => setDecisionModalOpen(true)}
        />

        <main className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
          {activeView === "devices" ? (
            <DevicesView
              workspace={workspace}
              status={status}
              onGoToWorkspace={() => setActiveView("tasks")}
            />
          ) : activeView === "settings" ? (
            <SettingsView
              activeSection={settingsSection}
              onSectionChange={setSettingsSection}
              theme={theme}
              onToggleTheme={toggleTheme}
              onGoToWorkspace={() => setActiveView("tasks")}
            />
          ) : !hasRepo ? (
            <WorkspaceWelcome
              recentWorkspaces={recentWorkspaces}
              isSwitching={switchWorkspace.isPending}
              onSwitchWorkspace={handleSwitchWorkspace}
              onRemoveRecentWorkspace={handleRemoveRecentWorkspace}
            />
          ) : activeView === "tasks" ? (
            <TaskWorkbench
              workspace={workspace}
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
              onPrimaryAction={handleTasksEmptyPrimaryAction}
              onRerun={openRun}
              onInitManifest={handleInitWorkspaceManifest}
              isInitializingManifest={initManifest.isPending}
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
              hasRepo={hasRepo}
              isGitRepo={Boolean(workspace?.is_git_repo)}
              hasManifest={Boolean(workspace?.has_manifest)}
              isGeneratingManifest={initManifest.isPending}
              onToggleIncludeStale={() => setIncludeStale((prev) => !prev)}
              onSelectNode={handleSelectNode}
              onOpenRecordDecision={() => setDecisionModalOpen(true)}
              onOpenDispatchTask={() => openRun()}
              onOpenRepoSettings={() => openSettingsSection("workspace")}
              manifestError={initManifest.error ? formatErrorMessage(initManifest.error, t) : null}
              onDismissManifestError={() => initManifest.reset()}
              onGenerateManifest={() =>
                initManifest.mutate({ repoPath: workspace?.repo_root ?? null })
              }
            />
          )}
        </main>
      </div>

      <RightWorkbenchRail
        isOpen={rightRailOpen}
        activeTab={rightRailTab}
        onTabChange={setRightRailTab}
        onClose={() => {
          setRightRailOpen(false);
          setSelectedNodeId(null);
          setSelectedTaskId(null);
        }}
        workspace={workspace}
        status={status}
        headCommitSha={dag?.head_commit_sha ?? null}
        staleCount={dag?.stale_count ?? 0}
        attentionCount={filterCounts.attention}
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
        onOpenLlmSettings={() => openSettingsSection("llm")}
      />

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
        onOpenRepoSettings={() => openSettingsSection("workspace")}
      />
    </div>
  );
}
