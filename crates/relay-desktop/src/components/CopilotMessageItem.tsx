import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent, CodeBlockLine } from "@/components/ui/code-block";
import {
  Item,
  ItemActions,
  ItemDescription,
  ItemFooter,
  ItemHeader,
  ItemTitle,
} from "@/components/ui/item";
import { Marker, MarkerContent, MarkerIcon } from "@/components/ui/marker";
import { Message, MessageContent } from "@/components/ui/message";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { cjk } from "@streamdown/cjk";
import {
  Activity,
  AlertCircle,
  CheckCircle2,
  FileCode2,
  FileText,
  GitBranch,
  GitCompare,
  History,
  Play,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  StickyNote,
  Terminal,
  Wrench,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Streamdown } from "streamdown";
import type {
  AgentApprovalCard_Serialize,
  AgentChatMessage_Serialize,
  AgentToolTrace_Serialize,
} from "../bindings";
import { useQuery } from "@tanstack/react-query";
import { formatErrorMessage } from "../lib/formatError";
import { relayKeys } from "../lib/queries";
import { relayApi, type ApprovalInput } from "../lib/relayApi";

const STREAMDOWN_PLUGINS = { cjk };

function toolIcon(toolName: string) {
  switch (toolName) {
    case "get_dag_snapshot":
      return <GitBranch />;
    case "inspect_sync_status":
      return <Activity />;
    case "list_handoffs":
      return <History />;
    case "check_env_diff":
      return <Terminal />;
    case "read_evidence_log":
      return <FileText />;
    case "read_repo_file":
    case "read_worktree_file":
      return <FileCode2 />;
    case "git_diff":
      return <GitCompare />;
    case "submit_task":
    case "run_preflight_in_worktree":
      return <Play />;
    case "propose_worktree_patch":
      return <Wrench />;
    case "approve_and_execute_task":
      return <ShieldCheck />;
    case "record_decision":
      return <StickyNote />;
    default:
      return <Terminal />;
  }
}

type ToolI18nKey =
  | "copilot.tools.inspect_sync_status"
  | "copilot.tools.list_handoffs"
  | "copilot.tools.check_env_diff"
  | "copilot.tools.get_dag_snapshot"
  | "copilot.tools.read_evidence_log"
  | "copilot.tools.run_preflight_in_worktree"
  | "copilot.tools.read_worktree_file"
  | "copilot.tools.propose_worktree_patch"
  | "copilot.tools.approve_and_execute_task"
  | "copilot.tools.submit_task"
  | "copilot.tools.record_decision"
  | "copilot.tools.read_repo_file"
  | "copilot.tools.git_diff";

const KNOWN_TOOL_KEYS: Record<string, ToolI18nKey> = {
  inspect_sync_status: "copilot.tools.inspect_sync_status",
  list_handoffs: "copilot.tools.list_handoffs",
  check_env_diff: "copilot.tools.check_env_diff",
  get_dag_snapshot: "copilot.tools.get_dag_snapshot",
  read_evidence_log: "copilot.tools.read_evidence_log",
  run_preflight_in_worktree: "copilot.tools.run_preflight_in_worktree",
  read_worktree_file: "copilot.tools.read_worktree_file",
  propose_worktree_patch: "copilot.tools.propose_worktree_patch",
  approve_and_execute_task: "copilot.tools.approve_and_execute_task",
  submit_task: "copilot.tools.submit_task",
  record_decision: "copilot.tools.record_decision",
  read_repo_file: "copilot.tools.read_repo_file",
  git_diff: "copilot.tools.git_diff",
};

function CopilotToolRow({ trace }: { trace: AgentToolTrace_Serialize }) {
  const { t } = useTranslation();
  const knownKey = KNOWN_TOOL_KEYS[trace.tool_name];
  const label = knownKey ? t(knownKey) : trace.tool_name;
  const isRunning = trace.status === "running";
  const statusLabel = trace.status === "error" ? t("copilot.tools.failed", { name: label }) : label;
  const lineStatus = isRunning ? "running" : trace.status === "error" ? "error" : undefined;

  const line = (
    <TraceLine status={lineStatus}>
      <TraceIcon>{toolIcon(trace.tool_name)}</TraceIcon>
      <TraceTitle>
        {isRunning ? t("copilot.tools.runningPrefix", { name: label }) : statusLabel}
      </TraceTitle>
      {trace.duration_ms != null && <TraceDescription>{trace.duration_ms}ms</TraceDescription>}
    </TraceLine>
  );

  // Per shadcn chat rules: a tool row is a plain TraceLine until output arrives.
  if (trace.output_preview == null) {
    return line;
  }

  const detailText =
    trace.args_preview && trace.args_preview !== "{}"
      ? `# ${trace.tool_name}(${trace.args_preview})\n${trace.output_preview}`
      : `# ${trace.tool_name}\n${trace.output_preview}`;

  return (
    <TraceCollapsible>
      <TraceCollapsibleTrigger>{line}</TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        <CodeBlock size="sm" variant={trace.status === "error" ? "destructive" : "default"}>
          <CodeBlockContent wrap className="max-h-56 text-[11px]">
            {detailText}
          </CodeBlockContent>
        </CodeBlock>
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

interface ParsedDiffLine {
  id: string;
  variant: "added" | "removed" | "default";
  text: string;
}

function parseDiffLines(diff: string): ParsedDiffLine[] {
  if (diff.trim().length === 0) return [];
  const counts = new Map<string, number>();
  return diff.split("\n").map((rawLine) => {
    const isAdded = rawLine.startsWith("+") && !rawLine.startsWith("+++");
    const isRemoved = rawLine.startsWith("-") && !rawLine.startsWith("---");
    const variant: ParsedDiffLine["variant"] = isAdded
      ? "added"
      : isRemoved
        ? "removed"
        : "default";
    const text = isAdded || isRemoved ? rawLine.slice(1) : rawLine;
    const seen = (counts.get(rawLine) ?? 0) + 1;
    counts.set(rawLine, seen);
    return {
      id: `${variant}:${rawLine}:${seen}`,
      variant,
      text,
    };
  });
}

const APPROVAL_BADGES = {
  approved: { variant: "secondary", label: "copilot.approvalApproved" },
  rejected: { variant: "outline", label: "copilot.approvalRejected" },
  pending: { variant: "default", label: "copilot.approvalPending" },
} as const;

function ResolvedApprovalNote({
  approved,
  isPatchFix,
}: {
  approved: boolean;
  isPatchFix: boolean;
}) {
  const { t } = useTranslation();
  return (
    <Marker className="px-0 py-0 text-[11.5px]">
      <MarkerIcon>
        {approved ? (
          <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" />
        ) : (
          <XCircle className="text-muted-foreground" />
        )}
      </MarkerIcon>
      <MarkerContent>
        {approved
          ? t("copilot.approvalApprovedNote")
          : t(isPatchFix ? "copilot.approvalRejectedNote" : "copilot.approvalRunRejectedNote")}
      </MarkerContent>
    </Marker>
  );
}

function InlineApprovalCard({
  approval,
  onResolveApproval,
  isResolvingApproval,
}: {
  approval: AgentApprovalCard_Serialize;
  onResolveApproval: (approval: ApprovalInput) => void;
  isResolvingApproval: boolean;
}) {
  const { t } = useTranslation();
  const isPatchFix = approval.patch_id != null;
  const { data: preset } = useQuery({
    queryKey: relayKeys.tasks(),
    queryFn: relayApi.listTasks,
    enabled: false, // The workspace owns the task query; observe its cached records.
    select: (tasks) =>
      tasks.find((task) => task.spec.task_id === approval.task_id)?.spec.command_preset,
  });
  const decision = approval.resolved;
  const badge =
    APPROVAL_BADGES[decision === "approved" || decision === "rejected" ? decision : "pending"];
  const diffLines = parseDiffLines(approval.diff_preview);

  return (
    <Item variant="outline" className="mt-1 flex-col items-stretch gap-2.5 bg-muted/30 p-3">
      <ItemHeader>
        <ItemTitle className="text-xs font-semibold text-foreground">
          <ShieldCheck className="size-3.5 text-primary" />
          <span>{t("copilot.approvalTitle")}</span>
        </ItemTitle>
        <Badge variant={badge.variant} className="h-5 px-1.5 text-[10.5px]">
          <span className="relay-text-trim leading-none">{t(badge.label)}</span>
        </Badge>
      </ItemHeader>

      <ItemDescription className="line-clamp-none text-xs leading-relaxed text-foreground">
        {!isPatchFix && preset
          ? t("copilot.cleanRunSummary", { preset })
          : approval.summary || t("detail.cleanRun")}
      </ItemDescription>

      {diffLines.length > 0 && (
        <CodeBlock size="sm" className="bg-background">
          <CodeBlockContent className="max-h-48 text-[11px]">
            {diffLines.map((line) => (
              <CodeBlockLine key={line.id} variant={line.variant}>
                {line.text}
              </CodeBlockLine>
            ))}
          </CodeBlockContent>
        </CodeBlock>
      )}

      {approval.resolved == null ? (
        <ItemFooter className="pt-0.5">
          <ItemActions className="w-full justify-end">
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={isResolvingApproval}
              onClick={() =>
                onResolveApproval({
                  taskId: approval.task_id,
                  patchId: approval.patch_id,
                  decision: "rejected",
                })
              }
            >
              <span className="relay-text-trim leading-none">
                {t(isPatchFix ? "detail.dismissFix" : "detail.cancelRun")}
              </span>
            </Button>
            <Button
              type="button"
              variant="default"
              size="sm"
              disabled={isResolvingApproval}
              onClick={() =>
                onResolveApproval({
                  taskId: approval.task_id,
                  patchId: approval.patch_id,
                  decision: "approved",
                })
              }
            >
              <CheckCircle2 data-icon="inline-start" />
              <span className="relay-text-trim leading-none">
                {t(isPatchFix ? "detail.applyAndRun" : "detail.approveRun")}
              </span>
            </Button>
          </ItemActions>
        </ItemFooter>
      ) : (
        <ResolvedApprovalNote approved={decision === "approved"} isPatchFix={isPatchFix} />
      )}
    </Item>
  );
}

interface CopilotMessageItemProps {
  message: AgentChatMessage_Serialize;
  isStreaming: boolean;
  onResolveApproval: (approval: ApprovalInput) => void;
  isResolvingApproval: boolean;
  onOpenLlmSettings: () => void;
}

export function CopilotMessageItem({
  message,
  isStreaming,
  onResolveApproval,
  isResolvingApproval,
  onOpenLlmSettings,
}: CopilotMessageItemProps) {
  const { t } = useTranslation();

  if (message.role === "user") {
    return (
      <Message align="end">
        <MessageContent>
          <Bubble variant="default" align="end">
            <BubbleContent className="text-[13px] leading-relaxed whitespace-pre-wrap">
              {message.content}
            </BubbleContent>
          </Bubble>
        </MessageContent>
      </Message>
    );
  }

  const showThinking = isStreaming && message.content.length === 0 && message.traces.length === 0;

  return (
    <Message align="start">
      <MessageContent className="gap-2.5">
        {message.traces.length > 0 && (
          <Trace>
            {message.traces.map((trace) => (
              <CopilotToolRow key={trace.id} trace={trace} />
            ))}
          </Trace>
        )}

        {showThinking && (
          <Trace>
            <TraceLine status="running">
              <TraceIcon>
                <Sparkles />
              </TraceIcon>
              <TraceTitle>{t("copilot.thinking")}</TraceTitle>
            </TraceLine>
          </Trace>
        )}

        {message.content.length > 0 ? (
          <Streamdown
            plugins={STREAMDOWN_PLUGINS}
            mode={isStreaming ? "streaming" : "static"}
            isAnimating={isStreaming}
            caret={isStreaming ? "block" : undefined}
            className="relay-markdown text-[13px] leading-relaxed text-foreground [&>*:first-child]:mt-0 [&>*:last-child]:mb-0"
          >
            {message.content}
          </Streamdown>
        ) : (
          !isStreaming &&
          message.approval != null && (
            <p className="text-[13px] leading-relaxed text-muted-foreground">
              {t("copilot.approvalIntro")}
            </p>
          )
        )}

        {message.approval != null && (
          <InlineApprovalCard
            approval={message.approval}
            onResolveApproval={onResolveApproval}
            isResolvingApproval={isResolvingApproval}
          />
        )}

        {message.error != null && (
          <Alert
            variant="destructive"
            className="border-destructive/30 bg-[var(--relay-stale-soft)]"
          >
            <AlertCircle />
            <AlertTitle>{t("copilot.errorTitle")}</AlertTitle>
            <AlertDescription className="flex flex-col gap-2">
              <span className="break-words">{formatErrorMessage(message.error, t)}</span>
              <div>
                <Button type="button" variant="outline" size="xs" onClick={onOpenLlmSettings}>
                  <SlidersHorizontal data-icon="inline-start" />
                  <span className="relay-text-trim leading-none">
                    {t("copilot.configureModel")}
                  </span>
                </Button>
              </div>
            </AlertDescription>
          </Alert>
        )}
      </MessageContent>
    </Message>
  );
}
