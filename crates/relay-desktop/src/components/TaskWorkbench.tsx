import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import {
  AlertCircle,
  ArrowRight,
  CheckCircle2,
  Loader2,
  Play,
  RotateCw,
  Sparkles,
  Square,
  Terminal,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type {
  DagSnapshot_Serialize,
  GraphNode,
  PeerStatus_Serialize,
  TaskRecord_Serialize,
} from "../bindings";
import { formatTaskSummary } from "../lib/formatError";
import { needsTaskApproval } from "../lib/relayApi";
import { isTaskCancellable, isTaskRunning, taskStateBadgeProps } from "../lib/taskLogStream";

export type TaskFilter = "all" | "attention" | "active" | "history";

interface TaskWorkbenchProps {
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

export function TaskWorkbench({
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

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto overscroll-contain bg-[var(--relay-canvas)] [scrollbar-gutter:stable]">
      <div className="mx-auto flex w-full max-w-4xl flex-1 flex-col gap-4 px-6 py-5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div
            role="group"
            aria-label={t("desk.filterLabel")}
            className="flex flex-wrap items-center gap-1.5"
          >
            {FILTER_ORDER.map((key) => {
              const active = filter === key;
              const count = counts[key];
              return (
                <Button
                  key={key}
                  type="button"
                  size="sm"
                  variant={active ? "secondary" : "ghost"}
                  aria-pressed={active}
                  onClick={() => onFilterChange(key)}
                  className={cn(
                    "gap-1.5 rounded-md",
                    active
                      ? "border border-border bg-card text-foreground shadow-2xs hover:bg-card"
                      : "text-muted-foreground",
                  )}
                >
                  <span className="relay-text-trim leading-none">{t(`desk.${key}`)}</span>
                  {count > 0 && (
                    <span
                      className={cn(
                        "font-mono-tech inline-flex min-w-4 items-center justify-center rounded-sm px-1 text-[10.5px] leading-4",
                        key === "attention"
                          ? "bg-primary/15 text-primary"
                          : "bg-muted text-muted-foreground",
                      )}
                    >
                      {count}
                    </span>
                  )}
                </Button>
              );
            })}
          </div>

          {showOfflineSyncNote && (
            <span
              role="status"
              className="inline-flex items-center gap-1.5 rounded-md border border-amber-500/25 bg-amber-500/10 px-2.5 py-1 text-[11.5px] text-amber-800 dark:text-amber-300"
            >
              <span className="size-1.5 shrink-0 rounded-full bg-amber-500" aria-hidden="true" />
              <span>{t("desk.recent")}</span>
            </span>
          )}
        </div>

        {errorText && (
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
        )}

        {isLoading ? (
          <div className="flex flex-1 items-center justify-center py-16 text-xs text-muted-foreground">
            <Loader2 className="mr-2 size-3.5 animate-spin" aria-hidden="true" />
            <span>{t("desk.loading")}</span>
          </div>
        ) : tasks.length === 0 ? (
          <div className="flex flex-1 items-center justify-center py-12">
            <div className="flex max-w-sm flex-col items-center gap-3 rounded-xl border border-dashed border-border bg-card/70 px-6 py-8 text-center">
              <p className="text-xs text-muted-foreground">
                {filter === "all" ? t("desk.empty") : t("desk.noMatch")}
              </p>
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
          </div>
        ) : (
          <ul role="list" className="flex flex-col gap-2.5">
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

              return (
                <li
                  key={task.spec.task_id}
                  className={cn(
                    "relative overflow-hidden rounded-lg border bg-card px-4 py-3 transition-colors",
                    needsFixReview
                      ? "border-primary/40 bg-[var(--relay-heal-soft)]"
                      : isSelected
                        ? "border-primary/50 ring-1 ring-primary/20"
                        : "border-border hover:border-foreground/20",
                  )}
                >
                  {running && (
                    <div
                      className="absolute inset-x-0 top-0 h-0.5 animate-pulse bg-amber-500"
                      aria-hidden="true"
                    />
                  )}

                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div className="flex min-w-0 items-center gap-2">
                      <button
                        type="button"
                        onClick={() => onInspectTask(task)}
                        className="cursor-pointer truncate rounded-sm text-left text-[13px] font-semibold text-foreground outline-none hover:text-primary focus-visible:ring-2 focus-visible:ring-ring/40"
                      >
                        {task.spec.command_preset}
                      </button>

                      <code
                        lang="en"
                        title={task.spec.base_commit_sha}
                        className="font-mono-tech shrink-0 rounded-sm bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground"
                      >
                        {task.spec.base_commit_sha.slice(0, 7)}
                      </code>

                      <time
                        dateTime={task.updated_at}
                        title={task.updated_at}
                        className="font-mono-tech hidden shrink-0 text-[11px] text-muted-foreground sm:inline"
                      >
                        {formatTaskTimestamp(task.updated_at, i18n.language)}
                      </time>
                    </div>

                    <div className="flex shrink-0 items-center gap-1.5">
                      {result && freshnessBadge && (
                        <Badge
                          variant={freshnessBadge.variant}
                          className={freshnessBadge.className}
                        >
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
                    </div>
                  </div>

                  {(task.preflight_summary || task.spec.user_note) && (
                    <p className="mt-1.5 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
                      {task.preflight_summary
                        ? formatTaskSummary(task.preflight_summary, t)
                        : task.spec.user_note}
                    </p>
                  )}

                  <div className="mt-2.5 flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-2">
                    <div className="flex flex-wrap items-center gap-3">
                      <button
                        type="button"
                        onClick={() => onInspectTask(task)}
                        className="inline-flex cursor-pointer items-center gap-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:underline"
                      >
                        <Terminal className="size-3 shrink-0" aria-hidden="true" />
                        <span className="relay-text-trim leading-none">
                          {t(running ? "sidebar.liveLogs" : "sidebar.viewLogs")}
                        </span>
                      </button>

                      {task.evidence_node_id && (
                        <button
                          type="button"
                          onClick={() => onSelectEvidenceNode(task.evidence_node_id!)}
                          className="inline-flex cursor-pointer items-center gap-1 text-xs font-medium text-primary hover:underline focus-visible:outline-none focus-visible:underline"
                        >
                          <CheckCircle2 className="size-3 shrink-0" aria-hidden="true" />
                          <span className="relay-text-trim leading-none">
                            {t("sidebar.viewResult")}
                          </span>
                        </button>
                      )}

                      {isStaleResult && (
                        <button
                          type="button"
                          onClick={() => onRerun(task)}
                          className="inline-flex cursor-pointer items-center gap-1 text-xs font-medium text-foreground transition-colors hover:text-primary focus-visible:outline-none focus-visible:underline"
                        >
                          <RotateCw className="size-3 shrink-0" aria-hidden="true" />
                          <span className="relay-text-trim leading-none">{t("desk.rerun")}</span>
                        </button>
                      )}
                    </div>

                    <div className="flex items-center gap-2">
                      {needsFixReview && (
                        <Button type="button" size="sm" onClick={() => onOpenApproval(task)}>
                          <Sparkles data-icon="inline-start" aria-hidden="true" />
                          <span className="relay-text-trim leading-none">
                            {t(task.active_proposal ? "sidebar.reviewFix" : "sidebar.reviewRun")}
                          </span>
                          <ArrowRight data-icon="inline-end" aria-hidden="true" />
                        </Button>
                      )}

                      {cancellable && (
                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          disabled={isCancelling}
                          onClick={() => onCancelTask(task.spec.task_id)}
                          className="h-6 gap-1 border-destructive/30 px-2 text-[11px] text-destructive hover:bg-destructive/10 hover:text-destructive"
                        >
                          {isCancelling ? (
                            <Loader2 className="size-3 shrink-0 animate-spin" aria-hidden="true" />
                          ) : (
                            <Square className="size-2.5 shrink-0" aria-hidden="true" />
                          )}
                          <span className="relay-text-trim leading-none">
                            {t(isCancelling ? "sidebar.cancellingTask" : "sidebar.cancelTask")}
                          </span>
                        </Button>
                      )}
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
