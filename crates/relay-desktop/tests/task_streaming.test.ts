import { expect, test } from "bun:test";
import type { TaskLogStreamEvent, TaskSpec_Deserialize, TaskState } from "../src/bindings";
import en from "../src/i18n/locales/en.json";
import zhCN from "../src/i18n/locales/zh-CN.json";
import { emitPreviewTaskLogEvent, relayApi, resetBrowserPreviewState } from "../src/lib/relayApi";
import {
  appendTaskLogChunk,
  clearTaskLogBuffer,
  isScrolledNearBottom,
  isTaskCancellable,
  isTaskRunning,
  nextAutoScrollOnUserScroll,
  renderTaskLogLines,
  splitLogChunksIntoLines,
  taskStateBadgeProps,
  type TaskLogBuffer,
} from "../src/lib/taskLogStream";

function makeChunk(
  overrides: Partial<TaskLogStreamEvent> & Pick<TaskLogStreamEvent, "seq" | "text">,
): TaskLogStreamEvent {
  return {
    task_id: overrides.task_id ?? "task-1",
    preset: overrides.preset ?? "train_grid",
    stream: overrides.stream ?? "stdout",
    seq: overrides.seq,
    text: overrides.text,
    timestamp: overrides.timestamp ?? "2026-01-01T00:00:00Z",
  };
}

function collectLeafKeys(value: unknown, prefix = ""): string[] {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return prefix ? [prefix] : [];
  }
  const keys: string[] = [];
  for (const [key, nested] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (nested && typeof nested === "object" && !Array.isArray(nested)) {
      keys.push(...collectLeafKeys(nested, path));
    } else {
      keys.push(path);
    }
  }
  return keys.sort();
}

test("appendTaskLogChunk deduplicates seq, orders out-of-order chunks, and caps buffer length", () => {
  let buffers: Record<string, TaskLogBuffer> = {};

  buffers = appendTaskLogChunk(buffers, makeChunk({ seq: 0, text: "line 0\n" }), 3);
  buffers = appendTaskLogChunk(buffers, makeChunk({ seq: 2, text: "line 2\n" }), 3);
  buffers = appendTaskLogChunk(buffers, makeChunk({ seq: 1, text: "line 1\n" }), 3);
  // Duplicate seq is ignored
  const afterDup = appendTaskLogChunk(buffers, makeChunk({ seq: 1, text: "dup\n" }), 3);
  expect(afterDup).toBe(buffers);
  expect(buffers["task-1"].chunks.map((c) => c.seq)).toEqual([0, 1, 2]);
  expect(buffers["task-1"].truncated).toBe(false);

  // Exceeding maxChunks drops oldest chunk and flags truncated
  buffers = appendTaskLogChunk(buffers, makeChunk({ seq: 3, text: "line 3\n" }), 3);
  expect(buffers["task-1"].chunks.map((c) => c.seq)).toEqual([1, 2, 3]);
  expect(buffers["task-1"].lastSeq).toBe(3);
  expect(buffers["task-1"].truncated).toBe(true);

  // Isolates multiple tasks and supports clearing single or all tasks
  buffers = appendTaskLogChunk(buffers, makeChunk({ task_id: "task-2", seq: 0, text: "other\n" }));
  expect(Object.keys(buffers).sort()).toEqual(["task-1", "task-2"]);
  buffers = clearTaskLogBuffer(buffers, "task-1");
  expect(Object.keys(buffers)).toEqual(["task-2"]);
  buffers = clearTaskLogBuffer(buffers);
  expect(buffers).toEqual({});
});

test("splitLogChunksIntoLines preserves stdout/stderr separation and normalizes CRLF", () => {
  const lines = splitLogChunksIntoLines([
    makeChunk({ seq: 0, stream: "stdout", text: "alpha\r\nbeta\r\n" }),
    makeChunk({ seq: 1, stream: "stderr", text: "warn: slow\n" }),
    makeChunk({ seq: 2, stream: "stdout", text: "partial without newline" }),
  ]);

  expect(lines).toEqual([
    { key: "0-0", seq: 0, stream: "stdout", text: "alpha" },
    { key: "0-1", seq: 0, stream: "stdout", text: "beta" },
    { key: "1-0", seq: 1, stream: "stderr", text: "warn: slow" },
    { key: "2-0", seq: 2, stream: "stdout", text: "partial without newline" },
  ]);
});

test("auto-scroll helpers pause when user scrolls up and stay paused until resumed", () => {
  const atBottom = { scrollTop: 380, clientHeight: 200, scrollHeight: 590 };
  const scrolledUp = { scrollTop: 120, clientHeight: 200, scrollHeight: 590 };

  expect(isScrolledNearBottom(atBottom)).toBe(true);
  expect(isScrolledNearBottom(scrolledUp)).toBe(false);

  expect(nextAutoScrollOnUserScroll(true, atBottom)).toBe(true);
  expect(nextAutoScrollOnUserScroll(true, scrolledUp)).toBe(false);
  // Once paused, scrolling near bottom does not silently flip auto-scroll back on
  expect(nextAutoScrollOnUserScroll(false, atBottom)).toBe(false);
});

test("the persisted log restores missing lines without assigning them a stream", () => {
  const live = [makeChunk({ seq: 1, stream: "stderr", text: "last line\n" })];
  expect(renderTaskLogLines(live, null)[0].stream).toBe("stderr");
  const final = renderTaskLogLines(live, "first line\r\nlast line\r\n");
  expect(final.map((line) => line.text)).toEqual(["first line", "last line"]);
  expect(final.every((line) => line.stream === null)).toBe(true);
});

test("task cancellation and badge helpers cover every TaskState variant", () => {
  const allStates: TaskState[] = [
    "queued",
    "preflight_running",
    "self_healing_proposed",
    "executing",
    "completed",
    "failed",
    "stale",
    "rejected",
    "cancelled",
  ];
  for (const state of allStates) {
    const badge = taskStateBadgeProps(state);
    expect(badge.variant).toBeTruthy();
  }
  expect(isTaskCancellable("queued")).toBe(true);
  expect(isTaskCancellable("preflight_running")).toBe(true);
  expect(isTaskCancellable("executing")).toBe(true);
  expect(isTaskCancellable("self_healing_proposed")).toBe(false);
  expect(isTaskCancellable("completed")).toBe(false);
  expect(isTaskCancellable("cancelled")).toBe(false);

  expect(isTaskRunning("preflight_running")).toBe(true);
  expect(isTaskRunning("executing")).toBe(true);
  expect(isTaskRunning("queued")).toBe(false);
  expect(isTaskRunning("cancelled")).toBe(false);
});

test("preview mode streams task logs, cancels cleanly, and preserves partial logs", async () => {
  resetBrowserPreviewState();
  const received: TaskLogStreamEvent[] = [];
  const unsubscribe = await relayApi.subscribeTaskLogEvents((event) => received.push(event));
  try {
    const spec: TaskSpec_Deserialize = {
      task_id: "stream-preview-task",
      repo_id: "sample",
      base_commit_sha: "abc123",
      command_preset: "train_grid",
      params: {},
      require_approval: false,
      requested_by_device: "preview",
    };
    const submitted = await relayApi.submitTask(spec);
    expect(submitted.state).toBe("queued");

    emitPreviewTaskLogEvent(
      makeChunk({ task_id: spec.task_id, seq: 0, stream: "stdout", text: "epoch 1\n" }),
    );
    emitPreviewTaskLogEvent(
      makeChunk({ task_id: spec.task_id, seq: 1, stream: "stderr", text: "warning\n" }),
    );
    expect(received.length).toBe(2);

    const cancelled = await relayApi.cancelTask(spec.task_id);
    expect(cancelled.state).toBe("cancelled");

    const persisted = await relayApi.getTaskLog(spec.task_id);
    expect(persisted?.content).toBe("epoch 1\nwarning\n");

    await expect(relayApi.cancelTask(spec.task_id)).rejects.toThrow("no longer be cancelled");
  } finally {
    unsubscribe();
    resetBrowserPreviewState();
  }
});

test("en.json and zh-CN.json maintain exact key symmetry under SyncEverything branding", () => {
  expect(en.app.name).toBe("SyncEverything");
  expect(zhCN.app.name).toBe("SyncEverything");
  expect(collectLeafKeys(en)).toEqual(collectLeafKeys(zhCN));
});
