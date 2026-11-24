import { expect, test } from "bun:test";
import type { AgentChatStreamEvent_Serialize } from "../src/bindings";
import {
  findCommitArtifact,
  needsTaskApproval,
  relayApi,
  resetBrowserPreviewState,
} from "../src/lib/relayApi";

test("clearing an active preview turn leaves no messages or late events", async () => {
  resetBrowserPreviewState();
  const events: AgentChatStreamEvent_Serialize[] = [];
  const unsubscribe = await relayApi.subscribeAgentChatEvents((event) => events.push(event));
  try {
    await relayApi.sendChatMessage("Inspect the workspace");
    expect(events.some((event) => event.kind === "tool_started")).toBe(true);
    await relayApi.clearChatMessages();
    const count = events.length;
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(events.length).toBe(count);
    expect(await relayApi.listChatMessages()).toEqual([]);
  } finally {
    unsubscribe();
    resetBrowserPreviewState();
  }
});

test("selects the current snapshot when a checkout revisits a commit", async () => {
  const dag = await relayApi.getDagSnapshot(true);
  const original = findCommitArtifact(dag, dag.head_commit_sha)!;
  const revisit = { ...original, id: "a-revisited" };
  const unrelated = {
    ...original,
    id: "z-unrelated",
    payload: { ...original.payload, git_commit_sha: "another" },
  };
  dag.nodes = [revisit, original, unrelated];
  dag.edges = [{ id: "return", from: revisit.id, to: original.id, kind: "supersedes" }];
  expect(findCommitArtifact(dag, dag.head_commit_sha)?.id).toBe(revisit.id);
});

test("shows clean runs only after successful preflight and explicit approval request", async () => {
  const [original] = await relayApi.listTasks();
  const clean = {
    ...original,
    state: "preflight_running" as const,
    active_proposal: null,
    preflight_summary: "Smoke preflight passed",
    spec: { ...original.spec, require_approval: true },
  };
  expect(needsTaskApproval(clean)).toBe(true);
  expect(needsTaskApproval({ ...clean, preflight_summary: null })).toBe(false);
  expect(needsTaskApproval({ ...clean, spec: { ...clean.spec, require_approval: false } })).toBe(
    false,
  );
});

test("a stale approval card cannot resolve a different proposal for the same task", async () => {
  resetBrowserPreviewState();
  const [task] = await relayApi.listTasks();
  const taskId = task.spec.task_id;
  await expect(
    relayApi.resolvePatchApproval({ taskId, patchId: null, decision: "approved" }),
  ).rejects.toThrow();
  expect((await relayApi.listTasks())[0].state).toBe("self_healing_proposed");
  const result = await relayApi.resolvePatchApproval({
    taskId,
    patchId: task.active_proposal!.patch_id,
    decision: "rejected",
  });
  expect(result.state).toBe("rejected");
  resetBrowserPreviewState();
});
