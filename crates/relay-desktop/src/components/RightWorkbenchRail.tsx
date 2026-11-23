import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { FileSearch, Sparkles, SquarePen, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  AgentChatMessage_Serialize,
  ApprovalDecision,
  GraphNode,
  LlmSettingsView_Serialize,
  TaskRecord_Serialize,
} from "../bindings";
import { AgentCopilotPanel } from "./AgentCopilotPanel";
import { InspectorPanelBody } from "./InspectorDrawer";

export type RightRailTab = "agent" | "inspector";

interface RightWorkbenchRailProps {
  isOpen: boolean;
  activeTab: RightRailTab;
  onTabChange: (tab: RightRailTab) => void;
  onClose: () => void;
  // Inspector props
  inspectorMode: "node" | "fix";
  selectedNode: GraphNode | null;
  pendingFixTask: TaskRecord_Serialize | null;
  onResolveApproval: (taskId: string, decision: ApprovalDecision, fromTab: RightRailTab) => void;
  isResolvingApproval: boolean;
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
  pendingFixTask,
  onResolveApproval,
  isResolvingApproval,
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

  const hasPendingFix = pendingFixTask?.active_proposal != null;

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
              {hasPendingFix && (
                <span className="size-1.5 shrink-0 rounded-full bg-primary" aria-hidden="true" />
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

      {activeTab === "agent" ? (
        <AgentCopilotPanel
          messages={messages}
          isStreaming={isStreaming}
          streamingMessageId={streamingMessageId}
          sendError={sendError}
          llmSettings={llmSettings}
          onSendMessage={onSendMessage}
          onCancelTurn={onCancelTurn}
          onResolveApproval={(taskId, decision) => onResolveApproval(taskId, decision, "agent")}
          isResolvingApproval={isResolvingApproval}
          onOpenLlmSettings={onOpenLlmSettings}
        />
      ) : (
        <InspectorPanelBody
          mode={inspectorMode}
          selectedNode={selectedNode}
          pendingFixTask={pendingFixTask}
          onResolveApproval={(taskId, decision) => onResolveApproval(taskId, decision, "inspector")}
          isResolvingApproval={isResolvingApproval}
        />
      )}
    </aside>
  );
}
