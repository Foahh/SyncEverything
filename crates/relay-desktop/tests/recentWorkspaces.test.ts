import { expect, test } from "bun:test";
import en from "../src/i18n/locales/en.json";
import zhCN from "../src/i18n/locales/zh-CN.json";
import { relayApi, resetBrowserPreviewState } from "../src/lib/relayApi";
import {
  MAX_RECENT_WORKSPACES,
  detectPreviewPackageManager,
  filterRecentWorkspaces,
  isActiveWorkspacePath,
  normalizeWorkspacePathKey,
  sortAndCapRecentWorkspaces,
  truncateWorkspacePath,
  type WorkspaceEntryLike,
} from "../src/lib/workspaceSwitcher";

function collectLeafKeys(value: unknown, prefix = ""): string[] {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return [prefix];
  }
  const entries = Object.entries(value as Record<string, unknown>);
  const keys: string[] = [];
  for (const [key, nested] of entries) {
    const fullKey = prefix ? `${prefix}.${key}` : key;
    if (typeof nested === "object" && nested !== null && !Array.isArray(nested)) {
      keys.push(...collectLeafKeys(nested, fullKey));
    } else {
      keys.push(fullKey);
    }
  }
  return keys.sort();
}

test("sortAndCapRecentWorkspaces orders by last_opened_at DESC, deduplicates normalized paths, and caps at 10", () => {
  const entries: WorkspaceEntryLike[] = [];
  for (let i = 0; i < 12; i++) {
    entries.push({
      path: `/work/project-${String(i).padStart(2, "0")}`,
      repo_id: `project-${String(i).padStart(2, "0")}`,
      name: `project-${String(i).padStart(2, "0")}`,
      package_manager: "uv",
      last_opened_at: new Date(Date.UTC(2026, 10, 25, 10, i, 0)).toISOString(),
      exists: true,
      has_manifest: true,
    });
  }
  // Duplicate of project-05 with trailing slash and newer timestamp should win and move to top.
  entries.push({
    path: "/work/project-05/",
    repo_id: "project-05",
    name: "project-05",
    package_manager: "cargo",
    last_opened_at: new Date(Date.UTC(2026, 10, 25, 11, 0, 0)).toISOString(),
    exists: true,
    has_manifest: true,
  });

  const sorted = sortAndCapRecentWorkspaces(entries, MAX_RECENT_WORKSPACES);
  expect(sorted).toHaveLength(10);
  expect(sorted[0].repo_id).toBe("project-05");
  expect(sorted[0].package_manager).toBe("cargo");
  expect(sorted.filter((entry) => entry.repo_id === "project-05")).toHaveLength(1);
  expect(sorted[1].repo_id).toBe("project-11");
  expect(sorted.some((entry) => entry.repo_id === "project-00")).toBe(false);
  expect(sorted.some((entry) => entry.repo_id === "project-01")).toBe(false);
});

test("normalizeWorkspacePathKey and isActiveWorkspacePath handle Windows and POSIX variants", () => {
  expect(normalizeWorkspacePathKey("\\\\?\\C:\\Users\\Dev\\Repo\\")).toBe("c:\\users\\dev\\repo");
  expect(isActiveWorkspacePath("C:/Users/Dev/Repo/", "c:\\users\\dev\\repo")).toBe(true);
  expect(isActiveWorkspacePath("/home/dev/repo/", "/home/dev/repo")).toBe(true);
  expect(isActiveWorkspacePath("/home/dev/other", "/home/dev/repo")).toBe(false);
  expect(isActiveWorkspacePath("/home/dev/repo", null)).toBe(false);
});

test("filterRecentWorkspaces matches name, repo_id, path, and package_manager", () => {
  const entries: WorkspaceEntryLike[] = [
    {
      path: "/repos/cuda-trainer",
      repo_id: "cuda-trainer",
      name: "CUDA Trainer",
      package_manager: "uv",
      last_opened_at: "2026-11-25T10:00:00Z",
      exists: true,
      has_manifest: true,
    },
    {
      path: "/repos/relay-engine",
      repo_id: "relay-engine",
      name: "Relay Engine",
      package_manager: "cargo",
      last_opened_at: "2026-11-25T09:00:00Z",
      exists: true,
      has_manifest: true,
    },
  ];

  expect(filterRecentWorkspaces(entries, "")).toHaveLength(2);
  expect(filterRecentWorkspaces(entries, "cargo").map((item) => item.repo_id)).toEqual([
    "relay-engine",
  ]);
  expect(filterRecentWorkspaces(entries, "CUDA").map((item) => item.repo_id)).toEqual([
    "cuda-trainer",
  ]);
  expect(filterRecentWorkspaces(entries, "nonexistent")).toEqual([]);
});

test("truncateWorkspacePath keeps short paths intact and preserves root and leaf for long paths", () => {
  expect(truncateWorkspacePath("/short/repo", 42)).toBe("/short/repo");

  const longWin = "C:\\Users\\researcher\\Documents\\projects\\synceverything\\relay-demo";
  const truncatedWin = truncateWorkspacePath(longWin, 36);
  expect(truncatedWin.length).toBeLessThanOrEqual(36);
  expect(truncatedWin).toContain("C:");
  expect(truncatedWin).toContain("relay-demo");

  const longPosix = "/Users/developer/workspace/organization/monorepo/forecast-engine";
  const truncatedPosix = truncateWorkspacePath(longPosix, 36);
  expect(truncatedPosix.length).toBeLessThanOrEqual(36);
  expect(truncatedPosix).toContain("forecast-engine");
});

test("detectPreviewPackageManager infers expected preview ecosystems", () => {
  expect(detectPreviewPackageManager("/preview/rust-service")).toBe("cargo");
  expect(detectPreviewPackageManager("/preview/bun-frontend")).toBe("bun");
  expect(detectPreviewPackageManager("/preview/ml-forecast")).toBe("uv");
  expect(detectPreviewPackageManager("/preview/plain-project")).toBe("generic");
});

test("browser preview tracks recent workspaces, refreshes manifest metadata, caps at 10, and removes entries", async () => {
  resetBrowserPreviewState();
  try {
    expect(await relayApi.listRecentWorkspaces()).toEqual([]);

    const rustInfo = await relayApi.switchWorkspace("/preview/rust-core");
    expect(rustInfo.package_manager).toBe("cargo");
    expect(rustInfo.has_manifest).toBe(false);

    const initialized = await relayApi.initWorkspaceManifest("/preview/rust-core", "cargo");
    expect(initialized.has_manifest).toBe(true);
    expect(initialized.package_manager).toBe("cargo");

    let recents = await relayApi.listRecentWorkspaces();
    expect(recents).toHaveLength(1);
    expect(recents[0].has_manifest).toBe(true);
    expect(recents[0].package_manager).toBe("cargo");

    for (let i = 0; i < 11; i++) {
      await relayApi.switchWorkspace(`/preview/ml-repo-${i}`);
    }
    recents = await relayApi.listRecentWorkspaces();
    expect(recents).toHaveLength(MAX_RECENT_WORKSPACES);
    expect(recents[0].repo_id).toBe("ml-repo-10");
    expect(recents.some((entry) => entry.repo_id === "rust-core")).toBe(false);

    const afterRemove = await relayApi.removeRecentWorkspace("/preview/ml-repo-10");
    expect(afterRemove).toHaveLength(MAX_RECENT_WORKSPACES - 1);
    expect(afterRemove.some((entry) => entry.repo_id === "ml-repo-10")).toBe(false);
  } finally {
    resetBrowserPreviewState();
  }
});

test("en.json and zh-CN.json have strictly symmetrical keys and SyncEverything branding", () => {
  expect(en.app.name).toBe("SyncEverything");
  expect(zhCN.app.name).toBe("SyncEverything");
  expect(collectLeafKeys(en)).toEqual(collectLeafKeys(zhCN));
});
