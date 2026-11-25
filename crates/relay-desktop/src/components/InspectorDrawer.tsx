import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  CheckCircle2,
  ChevronRight,
  Loader2,
  ShieldCheck,
  Sparkles,
  Square,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { GraphNode, JsonValue, TaskLogSnapshot, TaskRecord_Serialize } from "../bindings";
import { formatDeviceOrigin, formatErrorMessage, formatTaskSummary } from "../lib/formatError";
import { needsTaskApproval, type ApprovalInput } from "../lib/relayApi";
import { isTaskCancellable, taskStateBadgeProps, type TaskLogBuffer } from "../lib/taskLogStream";
import { TaskLogViewer } from "./TaskLogViewer";

type ActiveProposal = NonNullable<TaskRecord_Serialize["active_proposal"]>;

export type InspectorMode = "node" | "fix" | "task";

interface InspectorDrawerProps {
  mode: "closed" | InspectorMode;
  selectedNode: GraphNode | null;
  selectedTask?: TaskRecord_Serialize | null;
  taskLogBuffer?: TaskLogBuffer;
  persistedTaskLog?: TaskLogSnapshot | null;
  pendingFixTask: TaskRecord_Serialize | null;
  onResolveApproval: (approval: ApprovalInput) => void;
  isResolvingApproval: boolean;
  onCancelTask?: (taskId: string) => void;
  isCancellingTask?: boolean;
  onOpenApproval?: (task: TaskRecord_Serialize) => void;
  onSelectEvidenceNode?: (nodeId: string) => void;
  onClose: () => void;
}

function UnifiedDiffViewer({ diff }: { diff: string }) {
  const lines = diff.split("\n");
  return (
    <div className="relay-code-dark font-mono-tech overflow-x-auto rounded-lg border border-border bg-[#0e131b] text-[11.5px] leading-relaxed text-slate-200">
      <div className="flex flex-col gap-0.5 p-2.5">
        {lines.map((line, index) => {
          const isAdd = line.startsWith("+") && !line.startsWith("+++");
          const isDel = line.startsWith("-") && !line.startsWith("---");
          const isHunk = line.startsWith("@@");
          const isHeader = line.startsWith("---") || line.startsWith("+++");

          return (
            <div
              key={`${index}-${line.slice(0, 12)}`}
              className={cn(
                "flex gap-2.5 rounded-sm px-1.5 whitespace-pre",
                isAdd
                  ? "bg-emerald-500/18 text-emerald-300"
                  : isDel
                    ? "bg-red-500/18 text-red-300"
                    : isHunk
                      ? "bg-indigo-500/20 text-indigo-300"
                      : isHeader
                        ? "text-slate-400"
                        : "text-slate-300",
              )}
            >
              <span className="w-5 shrink-0 select-none text-right text-slate-500">
                {index + 1}
              </span>
              <span>{line}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

const ACRONYMS = new Set([
  "mae",
  "rmse",
  "mse",
  "r2",
  "ms",
  "cpu",
  "gpu",
  "rtt",
  "lr",
  "cuda",
  "mps",
]);

function formatMetricLabel(key: string): string {
  return key
    .split("_")
    .map((word) =>
      ACRONYMS.has(word.toLowerCase())
        ? word.toUpperCase()
        : word.charAt(0).toUpperCase() + word.slice(1),
    )
    .join(" ");
}

function metricEntries(metrics: JsonValue): Array<[string, string]> {
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) return [];
  return Object.entries(metrics).map(([k, v]) => [
    formatMetricLabel(k),
    typeof v === "number" ? String(v) : JSON.stringify(v),
  ]);
}

function CleanRunApprovalBody() {
  const { t } = useTranslation();
  return (
    <Alert className="border-emerald-500/25 bg-[var(--relay-fresh-soft)] text-emerald-800 dark:text-emerald-200">
      <ShieldCheck className="text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
      <AlertDescription className="text-emerald-800 dark:text-emerald-200">
        {t("detail.cleanRun")}
      </AlertDescription>
    </Alert>
  );
}

function FixProposalBody({ proposal }: { proposal: ActiveProposal }) {
  const { t } = useTranslation();
  return (
    <>
      <p className="text-[13px] leading-relaxed text-foreground">{proposal.summary}</p>

      <Alert className="border-emerald-500/25 bg-[var(--relay-fresh-soft)] text-emerald-800 dark:text-emerald-200">
        <ShieldCheck className="text-emerald-600 dark:text-emerald-400" />
        <AlertDescription className="text-emerald-800 dark:text-emerald-200">
          {t("detail.fixSafeNote")}
        </AlertDescription>
      </Alert>

      <div className="flex flex-col gap-1.5">
        <div className="text-[11.5px] font-medium text-muted-foreground">
          {t("detail.fixChanges")}
        </div>
        <UnifiedDiffViewer diff={proposal.diff_preview} />
      </div>

      <details className="group rounded-lg border border-border bg-muted/40">
        <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-xs font-medium text-foreground">
          <span className="inline-flex items-center gap-1.5">
            <CheckCircle2 className="size-3.5 shrink-0 text-emerald-600 dark:text-emerald-400" />
            <span className="relay-text-trim leading-none">
              {proposal.smoke_verified ? t("detail.fixCheckPassed") : t("detail.fixCheckLog")}
            </span>
          </span>
          <ChevronRight className="size-3.5 text-muted-foreground transition-transform group-open:rotate-90" />
        </summary>
        <pre className="font-mono-tech border-t border-border px-3 py-2.5 text-[11px] leading-relaxed whitespace-pre-wrap break-words text-muted-foreground">
          {proposal.smoke_stdout_tail}
        </pre>
      </details>
    </>
  );
}

function NodePayloadSection({ node }: { node: GraphNode }) {
  const { t } = useTranslation();
  const { payload } = node;

  if (payload.kind === "decision") {
    return (
      <div className="flex flex-col gap-4">
        {payload.rationale.trim().length > 0 && (
          <div className="flex flex-col gap-1.5">
            <div className="text-[11.5px] font-medium text-muted-foreground">{t("detail.why")}</div>
            <p className="text-[13px] leading-relaxed text-foreground">{payload.rationale}</p>
          </div>
        )}

        {payload.constraints.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <div className="text-[11.5px] font-medium text-muted-foreground">
              {t("detail.rules")}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {payload.constraints.map((rule) => (
                <Badge key={rule} variant="secondary" className="h-auto py-0.5 text-[11px]">
                  <span className="relay-text-trim leading-none">{rule}</span>
                </Badge>
              ))}
            </div>
          </div>
        )}
      </div>
    );
  }

  if (payload.kind === "artifact") {
    return (
      <div className="flex flex-col gap-2.5">
        <div className="text-[11.5px] font-medium text-muted-foreground">{t("detail.files")}</div>
        <ul className="flex flex-col gap-1">
          {payload.changed_files.map((file) => (
            <li
              key={file}
              className="font-mono-tech rounded-md border border-border bg-muted/60 px-2.5 py-1.5 text-xs text-foreground"
            >
              {file}
            </li>
          ))}
        </ul>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2.5">
      <div className="text-[11.5px] font-medium text-muted-foreground">{t("detail.metrics")}</div>
      <div className="grid grid-cols-2 gap-2">
        {metricEntries(payload.metrics_json).map(([label, val]) => (
          <div key={label} className="rounded-lg border border-border bg-muted/50 p-2.5">
            <div className="text-[11px] text-muted-foreground">{label}</div>
            <div className="font-mono-tech mt-0.5 text-sm font-semibold text-foreground">{val}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function NodeTechnicalDetails({ node }: { node: GraphNode }) {
  const { t } = useTranslation();
  return (
    <details className="group rounded-lg border border-border bg-muted/40">
      <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-[11.5px] font-medium text-muted-foreground">
        <span className="relay-text-trim leading-none">{t("detail.techDetails")}</span>
        <ChevronRight className="size-3.5 transition-transform group-open:rotate-90" />
      </summary>
      <div className="flex flex-col gap-2 border-t border-border px-3 py-2.5 text-[11.5px]">
        <div className="flex justify-between gap-2">
          <span className="text-muted-foreground">{t("detail.device")}</span>
          <span className="font-mono-tech text-foreground">
            {formatDeviceOrigin(node.device_origin, t)}
          </span>
        </div>
        <div className="flex justify-between gap-2">
          <span className="text-muted-foreground">{t("detail.updated")}</span>
          <span className="font-mono-tech text-foreground">
            {new Date(node.updated_at).toLocaleTimeString()}
          </span>
        </div>
        {node.payload.kind === "artifact" && (
          <>
            <div className="flex justify-between gap-2">
              <span className="text-muted-foreground">{t("detail.commit")}</span>
              <span className="font-mono-tech truncate text-foreground">
                {node.payload.git_commit_sha}
              </span>
            </div>
            <div className="flex justify-between gap-2">
              <span className="text-muted-foreground">{t("detail.env")}</span>
              <span className="font-mono-tech truncate text-foreground">
                {node.payload.env_fingerprint}
              </span>
            </div>
          </>
        )}
        {node.payload.kind === "evidence" && (
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t("detail.logHash")}</span>
            <span className="font-mono-tech truncate text-foreground">
              {node.payload.log_blob_hash}
            </span>
          </div>
        )}
      </div>
    </details>
  );
}

function TaskInspectorBody({
  task,
  buffer,
  persistedLog,
}: {
  task: TaskRecord_Serialize;
  buffer: TaskLogBuffer | undefined;
  persistedLog: TaskLogSnapshot | null | undefined;
}) {
  const { t } = useTranslation();

  return (
    <>
      {task.error_message && (
        <Alert variant="destructive" className="border-destructive/30 bg-[var(--relay-stale-soft)]">
          <AlertCircle />
          <AlertTitle>{t("detail.taskError")}</AlertTitle>
          <AlertDescription>{formatErrorMessage(task.error_message, t)}</AlertDescription>
        </Alert>
      )}

      {task.preflight_summary && (
        <p className="text-[12.5px] leading-relaxed text-muted-foreground">
          {formatTaskSummary(task.preflight_summary, t)}
        </p>
      )}

      {task.spec.user_note && (
        <div className="flex flex-col gap-1">
          <div className="text-[11px] font-medium text-muted-foreground">
            {t("detail.taskNote")}
          </div>
          <p className="text-[12.5px] leading-relaxed text-foreground">{task.spec.user_note}</p>
        </div>
      )}

      <TaskLogViewer
        key={task.spec.task_id}
        taskState={task.state}
        waitingForApproval={needsTaskApproval(task)}
        buffer={buffer}
        persistedLog={persistedLog}
      />

      {task.state === "cancelled" && Boolean(persistedLog?.content) && (
        <p className="text-[11.5px] text-muted-foreground">{t("detail.partialLogSaved")}</p>
      )}

      <details className="group rounded-lg border border-border bg-muted/40">
        <summary className="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-[11.5px] font-medium text-muted-foreground">
          <span className="relay-text-trim leading-none">{t("detail.techDetails")}</span>
          <ChevronRight className="size-3.5 transition-transform group-open:rotate-90" />
        </summary>
        <div className="flex flex-col gap-2 border-t border-border px-3 py-2.5 text-[11.5px]">
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t("detail.taskPreset")}</span>
            <span className="font-mono-tech text-foreground">{task.spec.command_preset}</span>
          </div>
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t("detail.taskCommit")}</span>
            <span className="font-mono-tech truncate text-foreground">
              {task.spec.base_commit_sha}
            </span>
          </div>
          <div className="flex justify-between gap-2">
            <span className="text-muted-foreground">{t("detail.updated")}</span>
            <span className="font-mono-tech text-foreground">
              {new Date(task.updated_at).toLocaleTimeString()}
            </span>
          </div>
          {persistedLog?.log_blob_hash && (
            <div className="flex justify-between gap-2">
              <span className="text-muted-foreground">{t("detail.logHash")}</span>
              <span className="font-mono-tech truncate text-foreground">
                {persistedLog.log_blob_hash}
              </span>
            </div>
          )}
        </div>
      </details>
    </>
  );
}

export function InspectorPanelBody({
  mode,
  selectedNode,
  selectedTask = null,
  taskLogBuffer,
  persistedTaskLog,
  pendingFixTask,
  onResolveApproval,
  isResolvingApproval,
  onCancelTask,
  isCancellingTask = false,
  onOpenApproval,
  onSelectEvidenceNode,
}: Omit<InspectorDrawerProps, "onClose">) {
  const { t } = useTranslation();

  const proposal = pendingFixTask?.active_proposal ?? null;
  const isFixMode = mode === "fix" && pendingFixTask != null;
  const isTaskMode = mode === "task" && selectedTask != null;

  if (!isFixMode && !isTaskMode && !selectedNode) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 px-6 text-center">
        <h3 className="text-sm font-medium text-foreground">{t("detail.emptyTitle")}</h3>
        <p className="text-xs leading-relaxed text-muted-foreground">{t("detail.emptyDesc")}</p>
      </div>
    );
  }

  const taskBadge = isTaskMode ? taskStateBadgeProps(selectedTask.state) : null;
  const canCancelSelectedTask = isTaskMode && isTaskCancellable(selectedTask.state);
  const selectedTaskNeedsApproval = isTaskMode && needsTaskApproval(selectedTask);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between gap-2 border-b border-border px-4 py-2.5">
        <div className="min-w-0">
          <div className="text-[11px] font-medium text-muted-foreground">
            {isFixMode
              ? t(proposal ? "detail.fixTitle" : "detail.cleanRunTitle")
              : isTaskMode
                ? t("detail.taskTitle")
                : t(`canvas.kind.${selectedNode!.payload.kind}`)}
          </div>
          <h3 className="truncate text-sm font-semibold text-foreground">
            {isFixMode
              ? pendingFixTask.spec.command_preset
              : isTaskMode
                ? selectedTask.spec.command_preset
                : selectedNode!.label}
          </h3>
        </div>
        {isTaskMode && taskBadge && (
          <Badge variant={taskBadge.variant} className={cn("shrink-0", taskBadge.className)}>
            <span className="relay-text-trim leading-none">
              {selectedTaskNeedsApproval && !selectedTask.active_proposal
                ? t("sidebar.awaitingApproval")
                : t(`taskState.${selectedTask.state}`)}
            </span>
          </Badge>
        )}
      </div>

      <div className="flex flex-1 flex-col gap-4 overflow-y-auto p-4">
        {isFixMode ? (
          proposal ? (
            <FixProposalBody proposal={proposal} />
          ) : (
            <CleanRunApprovalBody />
          )
        ) : isTaskMode ? (
          <TaskInspectorBody
            task={selectedTask}
            buffer={taskLogBuffer}
            persistedLog={persistedTaskLog}
          />
        ) : (
          selectedNode && (
            <>
              {selectedNode.freshness.status === "stale" && (
                <Alert
                  variant="destructive"
                  className="border-destructive/30 bg-[var(--relay-stale-soft)]"
                >
                  <AlertCircle />
                  <AlertTitle>{t("detail.outdatedTitle")}</AlertTitle>
                  <AlertDescription>
                    {t("detail.outdatedDesc", { reason: selectedNode.freshness.reason })}
                  </AlertDescription>
                </Alert>
              )}
              <NodePayloadSection node={selectedNode} />
              <NodeTechnicalDetails node={selectedNode} />
            </>
          )
        )}
      </div>

      {isFixMode && (
        <div className="flex items-center gap-2 border-t border-border bg-muted/40 p-3.5">
          <Button
            variant="outline"
            size="lg"
            className="flex-1"
            disabled={isResolvingApproval}
            onClick={() =>
              onResolveApproval({
                taskId: pendingFixTask.spec.task_id,
                patchId: proposal?.patch_id ?? null,
                decision: "rejected",
              })
            }
          >
            <span className="relay-text-trim leading-none">
              {t(proposal ? "detail.dismissFix" : "detail.cancelRun")}
            </span>
          </Button>
          <Button
            variant="default"
            size="lg"
            className="flex-1"
            disabled={isResolvingApproval}
            onClick={() =>
              onResolveApproval({
                taskId: pendingFixTask.spec.task_id,
                patchId: proposal?.patch_id ?? null,
                decision: "approved",
              })
            }
          >
            <CheckCircle2 data-icon="inline-start" />
            <span className="relay-text-trim leading-none">
              {t(proposal ? "detail.applyAndRun" : "detail.approveRun")}
            </span>
          </Button>
        </div>
      )}

      {isTaskMode &&
        (canCancelSelectedTask ||
          (selectedTaskNeedsApproval && onOpenApproval) ||
          (selectedTask.evidence_node_id && onSelectEvidenceNode)) && (
          <div className="flex items-center gap-2 border-t border-border bg-muted/40 p-3.5">
            {canCancelSelectedTask && onCancelTask && (
              <Button
                variant="destructive"
                size="lg"
                className="flex-1"
                disabled={isCancellingTask}
                onClick={() => onCancelTask(selectedTask.spec.task_id)}
              >
                {isCancellingTask ? (
                  <Loader2 className="animate-spin" data-icon="inline-start" />
                ) : (
                  <Square data-icon="inline-start" />
                )}
                <span className="relay-text-trim leading-none">
                  {t(isCancellingTask ? "detail.cancellingTaskBtn" : "detail.cancelTaskBtn")}
                </span>
              </Button>
            )}

            {selectedTaskNeedsApproval && onOpenApproval && (
              <Button
                variant="default"
                size="lg"
                className="flex-1"
                onClick={() => onOpenApproval(selectedTask)}
              >
                <Sparkles data-icon="inline-start" />
                <span className="relay-text-trim leading-none">
                  {t(selectedTask.active_proposal ? "sidebar.reviewFix" : "sidebar.reviewRun")}
                </span>
              </Button>
            )}

            {selectedTask.evidence_node_id && onSelectEvidenceNode && (
              <Button
                variant="outline"
                size="lg"
                className="flex-1"
                onClick={() => onSelectEvidenceNode(selectedTask.evidence_node_id!)}
              >
                <CheckCircle2 data-icon="inline-start" />
                <span className="relay-text-trim leading-none">{t("sidebar.viewResult")}</span>
              </Button>
            )}
          </div>
        )}
    </div>
  );
}

export function InspectorDrawer({
  mode,
  selectedNode,
  selectedTask = null,
  taskLogBuffer,
  persistedTaskLog,
  pendingFixTask,
  onResolveApproval,
  isResolvingApproval,
  onCancelTask,
  isCancellingTask,
  onOpenApproval,
  onSelectEvidenceNode,
  onClose,
}: InspectorDrawerProps) {
  const { t } = useTranslation();

  if (mode === "closed") return null;

  const proposal = pendingFixTask?.active_proposal ?? null;
  const isFixMode = mode === "fix" && pendingFixTask != null;
  const isTaskMode = mode === "task" && selectedTask != null;

  if (!isFixMode && !isTaskMode && !selectedNode) return null;

  return (
    <aside className="z-20 flex h-full w-[380px] shrink-0 flex-col border-l border-border bg-card text-card-foreground shadow-xl">
      <div className="flex items-center justify-between gap-2 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <div className="text-[11px] font-medium text-muted-foreground">
            {isFixMode
              ? t(proposal ? "detail.fixTitle" : "detail.cleanRunTitle")
              : isTaskMode
                ? t("detail.taskTitle")
                : t(`canvas.kind.${selectedNode!.payload.kind}`)}
          </div>
          <h3 className="truncate text-sm font-semibold text-foreground">
            {isFixMode
              ? pendingFixTask.spec.command_preset
              : isTaskMode
                ? selectedTask.spec.command_preset
                : selectedNode!.label}
          </h3>
        </div>

        <Button
          size="icon"
          variant="ghost"
          aria-label={t("detail.close")}
          onClick={onClose}
          className="shrink-0"
        >
          <X />
        </Button>
      </div>

      <InspectorPanelBody
        mode={mode}
        selectedNode={selectedNode}
        selectedTask={selectedTask}
        taskLogBuffer={taskLogBuffer}
        persistedTaskLog={persistedTaskLog}
        pendingFixTask={pendingFixTask}
        onResolveApproval={onResolveApproval}
        isResolvingApproval={isResolvingApproval}
        onCancelTask={onCancelTask}
        isCancellingTask={isCancellingTask}
        onOpenApproval={onOpenApproval}
        onSelectEvidenceNode={onSelectEvidenceNode}
      />
    </aside>
  );
}
