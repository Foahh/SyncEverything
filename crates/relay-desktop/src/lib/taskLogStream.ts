import type { TaskLogStreamEvent, TaskLogStreamKind, TaskState } from "../bindings";

export const MAX_BUFFERED_LOG_CHUNKS_PER_TASK = 2000;
export const AUTO_SCROLL_THRESHOLD_PX = 24;

export interface TaskLogBuffer {
  taskId: string;
  preset: string;
  chunks: TaskLogStreamEvent[];
  lastSeq: number;
  truncated: boolean;
}

export interface ScrollMetrics {
  scrollTop: number;
  clientHeight: number;
  scrollHeight: number;
}

export interface RenderedLogLine {
  key: string;
  seq: number;
  stream: TaskLogStreamKind | null;
  text: string;
}

export function isTaskCancellable(state: TaskState): boolean {
  return state === "queued" || state === "preflight_running" || state === "executing";
}

export function isTaskRunning(state: TaskState): boolean {
  return state === "preflight_running" || state === "executing";
}

export function taskStateBadgeProps(state: TaskState): {
  variant: "default" | "secondary" | "destructive" | "outline";
  className?: string;
} {
  switch (state) {
    case "completed":
      return {
        variant: "secondary",
        className:
          "bg-emerald-500/12 text-emerald-700 dark:bg-emerald-500/20 dark:text-emerald-300",
      };
    case "self_healing_proposed":
      return { variant: "default" };
    case "queued":
    case "preflight_running":
    case "executing":
      return {
        variant: "secondary",
        className: "bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-300",
      };
    case "cancelled":
      return {
        variant: "outline",
        className: "border-muted-foreground/35 bg-muted/60 text-muted-foreground",
      };
    case "failed":
    case "stale":
    case "rejected":
      return { variant: "destructive" };
  }
}

export function appendTaskLogChunk(
  buffers: Record<string, TaskLogBuffer>,
  event: TaskLogStreamEvent,
  maxChunks = MAX_BUFFERED_LOG_CHUNKS_PER_TASK,
): Record<string, TaskLogBuffer> {
  const existing = Object.hasOwn(buffers, event.task_id) ? buffers[event.task_id] : undefined;
  if (!existing) {
    const chunks = maxChunks > 0 ? [event] : [];
    return {
      ...buffers,
      [event.task_id]: {
        taskId: event.task_id,
        preset: event.preset,
        chunks,
        lastSeq: event.seq,
        truncated: maxChunks === 0,
      },
    };
  }

  if (existing.chunks.some((chunk) => chunk.seq === event.seq)) {
    return buffers;
  }

  let nextChunks: TaskLogStreamEvent[];
  if (event.seq > existing.lastSeq) {
    nextChunks = [...existing.chunks, event];
  } else {
    nextChunks = [...existing.chunks, event].sort((left, right) => left.seq - right.seq);
  }

  let truncated = existing.truncated;
  if (nextChunks.length > maxChunks) {
    nextChunks = nextChunks.slice(nextChunks.length - maxChunks);
    truncated = true;
  }

  return {
    ...buffers,
    [event.task_id]: {
      taskId: existing.taskId,
      preset: event.preset || existing.preset,
      chunks: nextChunks,
      lastSeq: Math.max(existing.lastSeq, event.seq),
      truncated,
    },
  };
}

export function clearTaskLogBuffer(
  buffers: Record<string, TaskLogBuffer>,
  taskId?: string,
): Record<string, TaskLogBuffer> {
  if (!taskId) {
    return {};
  }
  if (!Object.hasOwn(buffers, taskId)) {
    return buffers;
  }
  const next = { ...buffers };
  delete next[taskId];
  return next;
}

export function isScrolledNearBottom(
  metrics: ScrollMetrics,
  thresholdPx = AUTO_SCROLL_THRESHOLD_PX,
): boolean {
  const distanceFromBottom = metrics.scrollHeight - (metrics.scrollTop + metrics.clientHeight);
  return distanceFromBottom <= thresholdPx;
}

export function nextAutoScrollOnUserScroll(
  currentAutoScroll: boolean,
  metrics: ScrollMetrics,
  thresholdPx = AUTO_SCROLL_THRESHOLD_PX,
): boolean {
  if (!currentAutoScroll) {
    return false;
  }
  return isScrolledNearBottom(metrics, thresholdPx);
}

export function splitLogChunksIntoLines(chunks: TaskLogStreamEvent[]): RenderedLogLine[] {
  const lines: RenderedLogLine[] = [];
  for (const chunk of chunks) {
    const normalized = chunk.text.replace(/\r\n/g, "\n");
    const parts = normalized.split("\n");
    // Drop the trailing empty element produced by a normal newline terminator.
    if (parts.length > 1 && parts[parts.length - 1] === "") {
      parts.pop();
    }
    for (let idx = 0; idx < parts.length; idx++) {
      lines.push({
        key: `${chunk.seq}-${idx}`,
        seq: chunk.seq,
        stream: chunk.stream,
        text: parts[idx],
      });
    }
  }
  return lines;
}

export function renderTaskLogLines(
  chunks: TaskLogStreamEvent[],
  persistedContent: string | null,
): RenderedLogLine[] {
  if (persistedContent == null) return splitLogChunksIntoLines(chunks);
  if (!persistedContent) return [];

  const parts = persistedContent.replace(/\r\n/g, "\n").split("\n");
  if (parts.at(-1) === "") parts.pop();
  return parts.map((text, index) => ({
    key: `persisted-${index}`,
    seq: index,
    stream: null,
    text,
  }));
}
