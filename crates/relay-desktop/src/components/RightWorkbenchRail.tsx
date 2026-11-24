import { Alert, AlertAction, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { AlertCircle, FileSearch, Sparkles, SquarePen, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  AgentChatMessage_Serialize,
  GraphNode,
  LlmSettingsView_Serialize,
  TaskLogSnapshot,
  TaskRecord_Serialize,
} from "../bindings";
import type { ApprovalInput } from "../lib/relayApi";
import { isTaskRunning, type TaskLogBuffer } from "../lib/taskLogStream";
import { AgentCopilotPanel } from "./AgentCopilotPanel";
import { InspectorPanelBody, type InspectorMode } from "./InspectorDrawer";

export type RightRailTab = "agent" | "inspector";

interface RightWorkbenchRailProps {
  isOpen: boolean;
  activeTab: RightRailTab;
  onTabChange: (tab: RightRailTab) => void;
  onClose: () => void;
  // Inspector props
  inspectorMode: InspectorMode;
  selectedNode: GraphNode | null;
  selectedTask: TaskRecord_Serialize | null;
  taskLogBuffer: TaskLogBuffer | undefined;
  persistedTaskLog: TaskLogSnapshot | null | undefined;
  pendingFixTask: TaskRecord_Serialize | null;
  onResolveApproval: (approval: ApprovalInput, fromTab: RightRailTab) => void;
  isResolvingApproval: boolean;
  approvalError: string | null;
  onDismissApprovalError?: () => void;
  onCancelTask: (taskId: string) => void;
  cancellingTaskId: string | null;
  cancelError: string | null;
  onDismissCancelError?: () => void;
  onOpenApproval: (task: TaskRecord_Serialize) => void;
  onSelectEvidenceNode: (nodeId: string) => void;
  // Copilot props
  messages: AgentChatMessage_Serialize[];
  isStreaming: boolean;
  streamingMessageId: string | null;
  sendError: string | null;
  llmSettings: LlmSettingsView_Serialize | undefined;
  onSendMessage: (prompt: string) => Promise<void>;
  onCancelTurn: () => Promise<void>;
  onClearChat: () => Promise<void>;
  isClearingChat: boolean;
  onOpenLlmSettings: () => void;
}

export function RightWorkbenchRail({
  isOpen,
  activeTab,
  onTabChange,
  onClose,
  inspectorMode,
  selectedNode,
  selectedTask,
  taskLogBuffer,
  persistedTaskLog,
  pendingFixTask,
  onResolveApproval,
  isResolvingApproval,
  approvalError,
  onDismissApprovalError,
  onCancelTask,
  cancellingTaskId,
  cancelError,
  onDismissCancelError,
  onOpenApproval,
  onSelectEvidenceNode,
  messages,
  isStreaming,
  streamingMessageId,
  sendError,
  llmSettings,
  onSendMessage,
  onCancelTurn,
  onClearChat,
  isClearingChat,
  onOpenLlmSettings,
}: RightWorkbenchRailProps) {
  const { t } = useTranslation();

  if (!isOpen) return null;

  const hasPendingFix = pendingFixTask != null;
  const hasRunningTask = selectedTask != null && isTaskRunning(selectedTask.state);
  const isCancellingSelected =
    selectedTask != null && cancellingTaskId === selectedTask.spec.task_id;

  return (
    <aside className="z-20 flex h-full w-[400px] shrink-0 flex-col border-l border-border bg-card text-card-foreground shadow-xl xl:w-[420px]">
      <div className="flex h-11 shrink-0 items-center justify-between gap-2 border-b border-border px-3">
        <Tabs
          value={activeTab}
          onValueChange={(val) => {
            if (val === "agent" || val === "inspector") {
              onTabChange(val);
            }
          }}
          className="gap-0"
        >
          <TabsList className="h-7">
            <TabsTrigger value="agent" className="gap-1.5 px-2.5">
              <Sparkles className="size-3.5 text-primary" />
              <span className="relay-text-trim leading-none">{t("copilot.tabAgent")}</span>
            </TabsTrigger>
            <TabsTrigger value="inspector" className="gap-1.5 px-2.5">
              <FileSearch className="size-3.5" />
              <span className="relay-text-trim leading-none">{t("copilot.tabDetails")}</span>
              {(hasPendingFix || hasRunningTask) && (
                <span
                  className={
                    hasRunningTask
                      ? "size-1.5 shrink-0 animate-pulse rounded-full bg-amber-500"
                      : "size-1.5 shrink-0 rounded-full bg-primary"
                  }
                  aria-hidden="true"
                />
              )}
            </TabsTrigger>
          </TabsList>
        </Tabs>

        <div className="flex items-center gap-1">
          {activeTab === "agent" && messages.length > 0 && (
            <Button
              type="button"
              size="icon-sm"
              variant="ghost"
              aria-label={t("copilot.newChat")}
              title={t("copilot.newChat")}
              disabled={isClearingChat}
              onClick={() => void onClearChat()}
            >
              <SquarePen />
            </Button>
          )}
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            aria-label={t("detail.close")}
            title={t("detail.close")}
            onClick={onClose}
          >
            <X />
          </Button>
        </div>
      </div>

      {approvalError && (
        <div className="px-4 pt-3">
          <Alert
            variant="destructive"
            className="border-destructive/30 bg-[var(--relay-stale-soft)]"
          >
            <AlertCircle />
            <AlertTitle>{t("detail.approvalError")}</AlertTitle>
            <AlertDescription>{approvalError}</AlertDescription>
            {onDismissApprovalError && (
              <AlertAction>
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  aria-label={t("detail.close")}
                  onClick={onDismissApprovalError}
                >
                  <X />
                </Button>
              </AlertAction>
            )}
          </Alert>
        </div>
      )}

      {cancelError && (
        <div className="px-4 pt-3">
          <Alert
            variant="destructive"
            className="border-destructive/30 bg-[var(--relay-stale-soft)]"
          >
            <AlertCircle />
            <AlertTitle>{t("detail.cancelError")}</AlertTitle>
            <AlertDescription>{cancelError}</AlertDescription>
            {onDismissCancelError && (
              <AlertAction>
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  aria-label={t("detail.close")}
                  onClick={onDismissCancelError}
                >
                  <X />
                </Button>
              </AlertAction>
            )}
          </Alert>
        </div>
      )}

      {activeTab === "agent" ? (
        <AgentCopilotPanel
          messages={messages}
          isStreaming={isStreaming}
          streamingMessageId={streamingMessageId}
          sendError={sendError}
          llmSettings={llmSettings}
          onSendMessage={onSendMessage}
          onCancelTurn={onCancelTurn}
          onResolveApproval={(approval) => onResolveApproval(approval, "agent")}
          isResolvingApproval={isResolvingApproval}
          onOpenLlmSettings={onOpenLlmSettings}
        />
      ) : (
        <InspectorPanelBody
          mode={inspectorMode}
          selectedNode={selectedNode}
          selectedTask={selectedTask}
          taskLogBuffer={taskLogBuffer}
          persistedTaskLog={persistedTaskLog}
          pendingFixTask={pendingFixTask}
          onResolveApproval={(approval) => onResolveApproval(approval, "inspector")}
          isResolvingApproval={isResolvingApproval}
          onCancelTask={onCancelTask}
          isCancellingTask={isCancellingSelected}
          onOpenApproval={onOpenApproval}
          onSelectEvidenceNode={onSelectEvidenceNode}
        />
      )}
    </aside>
  );
}
