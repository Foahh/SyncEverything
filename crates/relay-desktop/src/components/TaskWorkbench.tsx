import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  ArrowRight,
  CheckCircle2,
  Clock,
  FilePlus2,
  Loader2,
  Play,
  RotateCw,
  Sparkles,
  Square,
  Terminal,
  XCircle,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  DagSnapshot_Serialize,
  GraphNode,
  PeerStatus_Serialize,
  TaskRecord_Serialize,
  WorkspaceInfo_Serialize,
} from "../bindings";
import { formatTaskSummary } from "../lib/formatError";
import { needsTaskApproval } from "../lib/relayApi";
import { isTaskCancellable, isTaskRunning, taskStateBadgeProps } from "../lib/taskLogStream";

export type TaskFilter = "all" | "attention" | "active" | "history";

interface TaskWorkbenchProps {
  workspace: WorkspaceInfo_Serialize | undefined;
  status: PeerStatus_Serialize | undefined;
  dag?: DagSnapshot_Serialize;
  tasks: TaskRecord_Serialize[];
  filter: TaskFilter;
  counts: Record<TaskFilter, number>;
  isLoading: boolean;
  errorText: string | null;
  onRetryLoad: () => void;
  onFilterChange: (filter: TaskFilter) => void;
  emptyActionLabel: string;
  onPrimaryAction: () => void;
  onRerun: (task: TaskRecord_Serialize) => void;
  onInitManifest: () => Promise<void>;
  isInitializingManifest: boolean;
  selectedTaskId: string | null;
  cancellingTaskId: string | null;
  onOpenApproval: (task: TaskRecord_Serialize) => void;
  onInspectTask: (task: TaskRecord_Serialize) => void;
  onCancelTask: (taskId: string) => void;
  onSelectEvidenceNode: (nodeId: string) => void;
}

const FILTER_ORDER: readonly TaskFilter[] = ["all", "attention", "active", "history"];

function formatTaskTimestamp(iso: string, locale: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  try {
    return new Intl.DateTimeFormat(locale, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  } catch {
    return date.toLocaleString();
  }
}

function freshnessBadgeProps(status: GraphNode["freshness"]["status"]): {
  variant: "destructive" | "secondary";
  className?: string;
} {
  if (status === "stale") {
    return { variant: "destructive" };
  }
  if (status === "pending_verify") {
    return {
      variant: "secondary",
      className: "bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
    };
  }
  return {
    variant: "secondary",
    className: "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
  };
}

function TaskStatusIndicator({
  task,
  needsFixReview,
  running,
}: {
  task: TaskRecord_Serialize;
  needsFixReview: boolean;
  running: boolean;
}) {
  if (needsFixReview) {
    return <Sparkles className="size-3.5 shrink-0 text-primary" aria-hidden="true" />;
  }
  if (running) {
    return <Loader2 className="size-3.5 shrink-0 animate-spin text-amber-500" aria-hidden="true" />;
  }
  if (task.state === "completed") {
    return (
      <CheckCircle2
        className="size-3.5 shrink-0 text-emerald-600 dark:text-emerald-400"
        aria-hidden="true"
      />
    );
  }
  if (task.state === "failed" || task.state === "stale") {
    return <XCircle className="size-3.5 shrink-0 text-destructive" aria-hidden="true" />;
  }
  return <Clock className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />;
}

export function TaskWorkbench({
  workspace,
  status,
  dag,
  tasks,
  filter,
  counts,
  isLoading,
  errorText,
  onRetryLoad,
  onFilterChange,
  emptyActionLabel,
  onPrimaryAction,
  onRerun,
  onInitManifest,
  isInitializingManifest,
  selectedTaskId,
  cancellingTaskId,
  onOpenApproval,
  onInspectTask,
  onCancelTask,
  onSelectEvidenceNode,
}: TaskWorkbenchProps) {
  const { t, i18n } = useTranslation();

  const showOfflineSyncNote =
    status?.mode === "client" && (!status.connected || status.pending_outbox_count > 0);
  const needsManifest =
    Boolean(workspace?.repo_root) && Boolean(workspace?.is_git_repo) && !workspace?.has_manifest;

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-background">
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-2">
        <div
          role="group"
          aria-label={t("desk.filterLabel")}
          className="flex flex-wrap items-center gap-1"
        >
          {FILTER_ORDER.map((key) => {
            const active = filter === key;
            const count = counts[key];
            return (
              <button
                key={key}
                type="button"
                aria-pressed={active}
                onClick={() => onFilterChange(key)}
                className={cn(
                  "inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-md px-2.5 text-xs font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                  active
                    ? "bg-muted text-foreground"
                    : "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
                )}
              >
                <span className="relay-text-trim leading-none">{t(`desk.${key}`)}</span>
                {count > 0 && (
                  <span
                    className={cn(
                      "font-mono-tech inline-flex min-w-4 items-center justify-center rounded-sm px-1 text-[10.5px] leading-4",
                      key === "attention"
                        ? "bg-primary/15 text-primary"
                        : "bg-background/80 text-muted-foreground",
                    )}
                  >
                    {count}
                  </span>
                )}
              </button>
            );
          })}
        </div>

        {showOfflineSyncNote && (
          <span
            role="status"
            className="inline-flex items-center gap-1.5 rounded-md border border-amber-500/25 bg-amber-500/10 px-2 py-0.5 text-[11px] text-amber-800 dark:text-amber-300"
          >
            <span className="size-1.5 shrink-0 rounded-full bg-amber-500" aria-hidden="true" />
            <span>{t("desk.recent")}</span>
          </span>
        )}
      </div>

      {needsManifest && (
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-border bg-muted/40 px-4 py-2 text-xs">
          <div className="flex min-w-0 items-center gap-2 text-muted-foreground">
            <FilePlus2 className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
            <span className="truncate">{t("canvas.missingManifestBanner")}</span>
          </div>
          <Button
            type="button"
            size="xs"
            disabled={isInitializingManifest}
            onClick={() => void onInitManifest()}
            className="shrink-0"
          >
            <span className="relay-text-trim leading-none">{t("canvas.generateManifestBtn")}</span>
          </Button>
        </div>
      )}

      {errorText && (
        <div className="border-b border-border px-4 py-2.5">
          <Alert
            role="alert"
            variant="destructive"
            className="border-destructive/30 bg-[var(--relay-stale-soft)]"
          >
            <AlertCircle aria-hidden="true" />
            <AlertDescription className="flex items-center justify-between gap-3">
              <span className="min-w-0 break-words">{errorText}</span>
              <Button type="button" size="xs" variant="outline" onClick={onRetryLoad}>
                <span className="relay-text-trim leading-none">{t("desk.retry")}</span>
              </Button>
            </AlertDescription>
          </Alert>
        </div>
      )}

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain [scrollbar-gutter:stable]">
        {isLoading ? (
          <div className="flex items-center gap-2 px-6 py-8 text-xs text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
            <span>{t("desk.loading")}</span>
          </div>
        ) : tasks.length === 0 ? (
          <div className="flex flex-col items-start gap-3 px-6 py-10">
            <div className="flex flex-col gap-1">
              <p className="text-sm font-medium text-foreground">
                {filter === "all" ? t("desk.empty") : t("desk.noMatch")}
              </p>
              {filter === "all" && (
                <p className="text-xs text-muted-foreground">{t("desk.emptyHint")}</p>
              )}
            </div>
            {filter === "all" ? (
              <Button type="button" size="default" onClick={onPrimaryAction}>
                <Play data-icon="inline-start" aria-hidden="true" />
                <span className="relay-text-trim leading-none">{emptyActionLabel}</span>
              </Button>
            ) : (
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => onFilterChange("all")}
              >
                <span className="relay-text-trim leading-none">{t("desk.showAll")}</span>
              </Button>
            )}
          </div>
        ) : (
          <ul role="list" className="divide-y divide-border">
            {tasks.map((task) => {
              const result = dag?.nodes.find((node) => node.id === task.evidence_node_id);
              const freshnessBadge = result ? freshnessBadgeProps(result.freshness.status) : null;
              const needsFixReview = needsTaskApproval(task);
              const badge = taskStateBadgeProps(task.state);
              const running = isTaskRunning(task.state) && !needsFixReview;
              const cancellable = isTaskCancellable(task.state);
              const isCancelling = cancellingTaskId === task.spec.task_id;
              const isSelected = selectedTaskId === task.spec.task_id;
              const isStaleResult = result?.freshness.status === "stale";
              const subtitle = task.preflight_summary
                ? formatTaskSummary(task.preflight_summary, t)
                : task.spec.user_note;

              return (
                <li
                  key={task.spec.task_id}
                  className={cn(
                    "group relative flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2.5 transition-colors sm:flex-nowrap",
                    needsFixReview
                      ? "bg-[var(--relay-heal-soft)]/70 hover:bg-[var(--relay-heal-soft)]"
                      : isSelected
                        ? "bg-primary/[0.05]"
                        : "hover:bg-muted/45",
                  )}
                >
                  {isSelected && (
                    <span
                      aria-hidden="true"
                      className="absolute inset-y-0 left-0 w-0.5 bg-primary"
                    />
                  )}

                  <div className="flex min-w-0 flex-1 items-center gap-3">
                    <TaskStatusIndicator
                      task={task}
                      needsFixReview={needsFixReview}
                      running={running}
                    />

                    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <div className="flex min-w-0 items-center gap-2">
                        <button
                          type="button"
                          onClick={() => onInspectTask(task)}
                          className="cursor-pointer truncate rounded-sm text-left text-[13px] font-medium text-foreground outline-none hover:text-primary focus-visible:ring-2 focus-visible:ring-ring/40"
                        >
                          {task.spec.command_preset}
                        </button>
                        <code
                          lang="en"
                          title={task.spec.base_commit_sha}
                          className="font-mono-tech shrink-0 rounded-sm bg-muted px-1.5 py-0.5 text-[10.5px] text-muted-foreground"
                        >
                          {task.spec.base_commit_sha.slice(0, 7)}
                        </code>
                      </div>

                      {subtitle && (
                        <p className="truncate text-xs text-muted-foreground">{subtitle}</p>
                      )}
                    </div>
                  </div>

                  <div className="flex shrink-0 items-center gap-2">
                    {result && freshnessBadge && (
                      <Badge variant={freshnessBadge.variant} className={freshnessBadge.className}>
                        <span className="relay-text-trim leading-none">
                          {t(`desk.${result.freshness.status}`)}
                        </span>
                      </Badge>
                    )}

                    <Badge variant={badge.variant} className={badge.className}>
                      <span className="relay-text-trim leading-none">
                        {needsFixReview && !task.active_proposal
                          ? t("sidebar.awaitingApproval")
                          : t(`taskState.${task.state}`)}
                      </span>
                    </Badge>

                    <time
                      dateTime={task.updated_at}
                      title={task.updated_at}
                      className="font-mono-tech hidden w-24 shrink-0 text-right text-[11px] text-muted-foreground lg:inline"
                    >
                      {formatTaskTimestamp(task.updated_at, i18n.language)}
                    </time>

                    <div className="flex shrink-0 items-center gap-1 pl-1">
                      {needsFixReview && (
                        <Button type="button" size="xs" onClick={() => onOpenApproval(task)}>
                          <span className="relay-text-trim leading-none">
                            {t(task.active_proposal ? "sidebar.reviewFix" : "sidebar.reviewRun")}
                          </span>
                          <ArrowRight data-icon="inline-end" aria-hidden="true" />
                        </Button>
                      )}

                      {isStaleResult && (
                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          onClick={() => onRerun(task)}
                          title={t("desk.rerun")}
                        >
                          <RotateCw data-icon="inline-start" aria-hidden="true" />
                          <span className="relay-text-trim leading-none">
                            {t("desk.rerunShort")}
                          </span>
                        </Button>
                      )}

                      {task.evidence_node_id && (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          onClick={() => onSelectEvidenceNode(task.evidence_node_id!)}
                          className="text-primary hover:text-primary"
                        >
                          <span className="relay-text-trim leading-none">
                            {t("sidebar.viewResult")}
                          </span>
                        </Button>
                      )}

                      {cancellable && (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          disabled={isCancelling}
                          onClick={() => onCancelTask(task.spec.task_id)}
                          className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                        >
                          {isCancelling ? (
                            <Loader2 className="size-3 animate-spin" aria-hidden="true" />
                          ) : (
                            <Square className="size-2.5" aria-hidden="true" />
                          )}
                          <span className="relay-text-trim leading-none">
                            {t(isCancelling ? "sidebar.cancellingTask" : "sidebar.cancelTask")}
                          </span>
                        </Button>
                      )}

                      <Button
                        type="button"
                        size="icon-xs"
                        variant="ghost"
                        onClick={() => onInspectTask(task)}
                        aria-label={t(running ? "sidebar.liveLogs" : "sidebar.viewLogs")}
                        title={t(running ? "sidebar.liveLogs" : "sidebar.viewLogs")}
                        className="text-muted-foreground hover:text-foreground"
                      >
                        <Terminal />
                      </Button>
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
