import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ArrowDownToLine, Pause, Terminal } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TaskLogSnapshot, TaskState } from "../bindings";
import {
  isTaskRunning,
  nextAutoScrollOnUserScroll,
  renderTaskLogLines,
  type TaskLogBuffer,
} from "../lib/taskLogStream";

interface TaskLogViewerProps {
  taskState: TaskState;
  waitingForApproval: boolean;
  buffer: TaskLogBuffer | undefined;
  persistedLog: TaskLogSnapshot | null | undefined;
}

export function TaskLogViewer({
  taskState,
  waitingForApproval,
  buffer,
  persistedLog,
}: TaskLogViewerProps) {
  const { t } = useTranslation();
  const scrollContainerRef = useRef<HTMLDivElement | null>(null);
  const [autoScroll, setAutoScroll] = useState(true);

  const running = isTaskRunning(taskState) && !waitingForApproval;
  const chunks = buffer?.chunks;
  const persistedContent = running ? null : (persistedLog?.content ?? null);
  const lines = useMemo(
    () => renderTaskLogLines(chunks ?? [], persistedContent),
    [chunks, persistedContent],
  );

  useEffect(() => {
    if (!autoScroll) return;
    const el = scrollContainerRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [autoScroll, lines]);

  const handleScroll = () => {
    const el = scrollContainerRef.current;
    if (!el) return;
    const next = nextAutoScrollOnUserScroll(autoScroll, {
      scrollTop: el.scrollTop,
      clientHeight: el.clientHeight,
      scrollHeight: el.scrollHeight,
    });
    if (next !== autoScroll) {
      setAutoScroll(next);
    }
  };

  const handleToggleAutoScroll = () => {
    setAutoScroll((prev) => !prev);
  };

  const statusLabel = waitingForApproval
    ? t("sidebar.awaitingApproval")
    : running
      ? t("detail.taskStatusStreaming")
      : taskState === "queued"
        ? t("detail.taskStatusQueued")
        : taskState === "cancelled"
          ? t("detail.taskStatusCancelled")
          : t("detail.taskStatusFinished");

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <Terminal className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="text-[11.5px] font-medium text-muted-foreground">
            {t("detail.taskLogsTitle")}
          </span>
          <Badge
            variant="outline"
            className={cn(
              "h-5 gap-1 px-1.5 text-[10px]",
              running && "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
              taskState === "cancelled" &&
                "border-muted-foreground/35 bg-muted/60 text-muted-foreground",
            )}
          >
            {running && (
              <span
                className="size-1.5 shrink-0 animate-pulse rounded-full bg-amber-500"
                aria-hidden="true"
              />
            )}
            <span role="status" className="relay-text-trim leading-none">
              {statusLabel}
            </span>
          </Badge>
        </div>

        <Button
          type="button"
          size="xs"
          variant={autoScroll ? "outline" : "secondary"}
          aria-pressed={!autoScroll}
          onClick={handleToggleAutoScroll}
          className="h-6 gap-1 px-2 text-[11px]"
        >
          {autoScroll ? (
            <>
              <Pause className="size-3 shrink-0" />
              <span className="relay-text-trim leading-none">{t("detail.pauseAutoScroll")}</span>
            </>
          ) : (
            <>
              <ArrowDownToLine className="size-3 shrink-0" />
              <span className="relay-text-trim leading-none">{t("detail.resumeAutoScroll")}</span>
            </>
          )}
        </Button>
      </div>

      <div
        ref={scrollContainerRef}
        onScroll={handleScroll}
        role="log"
        aria-live="off"
        aria-label={t("detail.taskLogsTitle")}
        className="relay-code-dark font-mono-tech max-h-[360px] min-h-[160px] overflow-y-auto overscroll-contain rounded-lg border border-border bg-[#0e131b] text-[11.5px] leading-relaxed text-slate-200 [scrollbar-gutter:stable]"
      >
        {persistedContent == null && buffer?.truncated && (
          <div className="border-b border-slate-800 bg-slate-900/80 px-2.5 py-1 text-[10.5px] text-slate-400">
            {t("detail.logsTruncated")}
          </div>
        )}

        {lines.length === 0 ? (
          <div className="flex min-h-[156px] items-center justify-center px-3 py-6 text-center text-xs text-slate-400">
            {waitingForApproval
              ? t("detail.logsAwaitingApproval")
              : running || taskState === "queued"
                ? t("detail.noLogsYet")
                : t("detail.noLogsRecorded")}
          </div>
        ) : (
          <div className="flex flex-col gap-0.5 p-2.5">
            {lines.map((entry) => {
              const isStderr = entry.stream === "stderr";
              return (
                <div
                  key={entry.key}
                  className={cn(
                    "flex items-start gap-2 rounded-xs px-1.5 py-0.5 whitespace-pre-wrap break-all",
                    isStderr
                      ? "border-l-2 border-red-400/80 bg-red-500/14 text-red-200"
                      : "text-slate-200",
                  )}
                >
                  {entry.stream && (
                    <span
                      className={cn(
                        "w-7 shrink-0 select-none text-[9.5px] font-semibold tracking-wide uppercase",
                        isStderr ? "text-red-400" : "text-slate-500",
                      )}
                    >
                      {isStderr ? t("detail.stderrLabel") : t("detail.stdoutLabel")}
                    </span>
                  )}
                  <span className="min-w-0 flex-1">{entry.text || " "}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>

      {!autoScroll && lines.length > 0 && (
        <div className="text-[11px] text-muted-foreground">{t("detail.autoScrollPaused")}</div>
      )}
    </div>
  );
}
