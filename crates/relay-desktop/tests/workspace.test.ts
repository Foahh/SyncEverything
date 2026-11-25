import { expect, test } from "bun:test";
import type {
  AgentChatStreamEvent_Serialize,
  GraphNode,
  TaskSpec_Deserialize,
} from "../src/bindings";
import {
  findCommitArtifact,
  needsTaskApproval,
  relayApi,
  resetBrowserPreviewState,
  requireCurrentApproval,
} from "../src/lib/relayApi";

import i18n from "../src/i18n";
import { formatErrorMessage } from "../src/lib/formatError";
import { buildMcpSnippet } from "../src/lib/mcpConfig";

const testSpec: TaskSpec_Deserialize = {
  task_id: "test-task",
  repo_id: "sample",
  base_commit_sha: "abc123",
  command_preset: "check",
  params: {},
  require_approval: true,
  requested_by_device: "test",
};

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
  dag.head_commit_sha = testSpec.base_commit_sha;
  const original: GraphNode = {
    id: "original",
    label: "Initial commit",
    freshness: { status: "fresh" },
    payload: {
      kind: "artifact",
      git_commit_sha: testSpec.base_commit_sha,
      changed_files: [],
      working_tree_dirty: false,
      env_fingerprint: "test",
    },
    device_origin: "test",
    created_at: dag.captured_at,
    updated_at: dag.captured_at,
  };
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
  const original = await relayApi.submitTask(testSpec);
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
  const task = await relayApi.submitTask(testSpec);
  const taskId = task.spec.task_id;
  await expect(
    relayApi.resolvePatchApproval({ taskId, patchId: "expired-fix", decision: "approved" }),
  ).rejects.toThrow("changed");
  expect((await relayApi.listTasks())[0].state).toBe("queued");
  const current = {
    ...task,
    state: "self_healing_proposed" as const,
    active_proposal: {
      patch_id: "current-fix",
      task_id: taskId,
      kind: "uv_spec" as const,
      summary: "fix",
      diff_preview: "",
      worktree_branch: "task/test",
      smoke_verified: true,
      smoke_stdout_tail: "",
      created_at: task.created_at,
    },
  };
  expect(() => requireCurrentApproval(current, null)).toThrow("changed");
  expect(() => requireCurrentApproval(current, "expired-fix")).toThrow("changed");
  expect(() => requireCurrentApproval(current, current.active_proposal.patch_id)).not.toThrow();
  resetBrowserPreviewState();
});

test("MCP clients reuse the desktop repository and data paths verbatim", () => {
  const workspace = {
    repo_id: "test",
    repo_root: "C:\\",
    mcp_data_dir: "D:\\Data Folder\\repos\\test",
  };
  const expected = [
    "--repo-root",
    workspace.repo_root,
    "--repo-id",
    workspace.repo_id,
    "--data-dir",
    workspace.mcp_data_dir,
    "mcp-stdio",
  ];
  for (const client of ["claude", "cursor"] as const) {
    expect(JSON.parse(buildMcpSnippet(client, workspace)!).mcpServers.relay.args).toEqual(expected);
  }
  const codex = Bun.TOML.parse(buildMcpSnippet("codex", workspace)!);
  expect(codex.mcp_servers.relay.args).toEqual(expected);
  expect(buildMcpSnippet("claude", { ...workspace, mcp_data_dir: null })).toBeNull();
  expect(buildMcpSnippet("codex")).toBeNull();
});

test("onboarding starts empty and preserves independent preview repositories", async () => {
  resetBrowserPreviewState();
  expect((await relayApi.getWorkspaceInfo()).repo_root).toBeNull();
  expect((await relayApi.getDagSnapshot(true)).nodes).toEqual([]);
  const path = "/preview/project";
  expect((await relayApi.openWorkspaceRepo(path)).presets).toEqual([]);
  const workspace = await relayApi.initWorkspaceManifest();
  expect(workspace.presets.map((preset) => preset.name)).toEqual(["check"]);
  await relayApi.submitTask(testSpec);
  await relayApi.openWorkspaceRepo("/preview/other");
  expect(await relayApi.listTasks()).toEqual([]);
  expect((await relayApi.openWorkspaceRepo(path)).presets).toEqual(workspace.presets);
  expect((await relayApi.listTasks()).map((task) => task.spec.task_id)).toEqual([testSpec.task_id]);
  resetBrowserPreviewState();
});

test("formatErrorMessage maps known backend errors, strips Error prefix, and falls back on empty", () => {
  const tZh = i18n.getFixedT("zh-CN");
  const tEn = i18n.getFixedT("en");

  expect(formatErrorMessage("Open a Git repository before running a task.", tZh)).toBe(
    tZh("errors.openRepoBeforeTask"),
  );
  expect(
    formatErrorMessage(
      "Error: git command failed: Create the first Git commit before running a task.",
      tZh,
    ),
  ).toBe(tZh("errors.createFirstCommit"));
  expect(
    formatErrorMessage(new Error("transport error: Choose a device role before pairing"), tEn),
  ).toBe(tEn("errors.chooseRoleBeforePairing"));
  expect(
    formatErrorMessage("internal error: relay.toml already exists in this repository", tZh),
  ).toBe(tZh("errors.manifestAlreadyExists"));

  expect(formatErrorMessage(new Error("Error: Network timeout on 192.168.1.10"), tZh)).toBe(
    "Network timeout on 192.168.1.10",
  );
  expect(formatErrorMessage("   ", tZh, "自定义兜底")).toBe("自定义兜底");
  expect(formatErrorMessage("", tZh)).toBe(tZh("errors.generic"));
  expect(formatErrorMessage(null, tEn)).toBe(tEn("errors.generic"));
});

test("preview Copilot reports live evidence authority after graph changes", async () => {
  resetBrowserPreviewState();
  const now = new Date().toISOString();
  const evidence: GraphNode = {
    id: "result",
    label: "result",
    device_origin: "test",
    created_at: now,
    updated_at: now,
    freshness: { status: "stale", marked_at: now, caused_by: "new-commit", reason: "code changed" },
    payload: {
      kind: "evidence",
      task_id: "task",
      preset: "check",
      metrics_json: { score: 42 },
      log_blob_hash: "log",
    },
  };
  await relayApi.upsertDagNode(evidence, []);
  const dag = await relayApi.getDagSnapshot(false);
  expect(dag.nodes).toEqual([]);
  expect(dag.stale_evidence_count).toBe(1);

  const { promise, resolve } = Promise.withResolvers<string>();
  const unsubscribe = await relayApi.subscribeAgentChatEvents((event) => {
    if (event.kind === "tool_finished" && event.trace?.output_preview)
      resolve(event.trace.output_preview);
  });
  try {
    await relayApi.sendChatMessage("Check outdated results");
    expect(JSON.parse(await promise)).toMatchObject({
      stale_count: 1,
      fresh_evidence_count: 0,
      stale_evidence_count: 1,
      metric_authority_note: dag.metric_authority_note,
    });
  } finally {
    unsubscribe();
    await relayApi.clearChatMessages();
    resetBrowserPreviewState();
  }
});

test("model settings preserve provider-auto separately from tier defaults and mask short preview keys", async () => {
  resetBrowserPreviewState();
  const initial = await relayApi.getLlmSettings();
  const settings = {
    tier: initial.tier,
    base_url: initial.base_url,
    model: initial.model,
    api_key_env: null,
  };
  const configured = await relayApi.saveLlmSettings({
    ...settings,
    reasoning_effort: "auto",
    api_key: "tiny",
  });
  expect(configured.reasoning_effort).toBe("auto");
  expect(configured.api_key_hint).toBe("••••••");
  expect(configured.has_api_key).toBe(true);
  const tierDefault = await relayApi.saveLlmSettings({ ...settings, reasoning_effort: null });
  expect(tierDefault.reasoning_effort).toBeNull();
  expect(tierDefault.api_key_hint).toBe(configured.api_key_hint);
  const cleared = await relayApi.saveLlmSettings({
    ...settings,
    api_key: "",
    api_key_env: "MODEL_KEY",
  });
  expect(cleared.has_api_key).toBe(false);
  resetBrowserPreviewState();
});
