export const MAX_RECENT_WORKSPACES = 10;

export interface WorkspaceEntryLike {
  path: string;
  repo_id: string;
  name: string;
  package_manager: string | null;
  last_opened_at: string;
  exists: boolean;
  has_manifest: boolean;
}

export function normalizeWorkspacePathKey(rawPath: string): string {
  let cleaned = rawPath.trim();
  if (cleaned.startsWith("\\\\?\\UNC\\")) {
    cleaned = `\\\\${cleaned.slice(8)}`;
  } else if (cleaned.startsWith("\\\\?\\")) {
    cleaned = cleaned.slice(4);
  }
  const isWindowsPath = /^[a-zA-Z]:[\\/]/.test(cleaned) || cleaned.startsWith("\\\\");
  if (isWindowsPath) {
    cleaned = cleaned.replaceAll("/", "\\");
    while (cleaned.length > 3 && cleaned.endsWith("\\")) {
      cleaned = cleaned.slice(0, -1);
    }
    return cleaned.toLowerCase();
  }
  cleaned = cleaned.replaceAll("\\", "/");
  while (cleaned.length > 1 && cleaned.endsWith("/")) {
    cleaned = cleaned.slice(0, -1);
  }
  return cleaned;
}

export function isActiveWorkspacePath(
  entryPath: string,
  activeRepoRoot: string | null | undefined,
): boolean {
  if (!activeRepoRoot) return false;
  return normalizeWorkspacePathKey(entryPath) === normalizeWorkspacePathKey(activeRepoRoot);
}

export function sortAndCapRecentWorkspaces<
  T extends Pick<WorkspaceEntryLike, "path" | "last_opened_at">,
>(entries: readonly T[], limit = MAX_RECENT_WORKSPACES): T[] {
  const sorted = [...entries].sort((a, b) => {
    const aMs = Date.parse(a.last_opened_at);
    const bMs = Date.parse(b.last_opened_at);
    if (!Number.isNaN(aMs) && !Number.isNaN(bMs) && aMs !== bMs) {
      return bMs - aMs;
    }
    const isoCmp = b.last_opened_at.localeCompare(a.last_opened_at);
    if (isoCmp !== 0) return isoCmp;
    return a.path.localeCompare(b.path);
  });

  const seen = new Set<string>();
  const deduped: T[] = [];
  for (const entry of sorted) {
    const key = normalizeWorkspacePathKey(entry.path);
    if (seen.has(key)) continue;
    seen.add(key);
    deduped.push(entry);
    if (deduped.length >= Math.max(0, limit)) break;
  }
  return deduped;
}

export function filterRecentWorkspaces<
  T extends Pick<WorkspaceEntryLike, "name" | "repo_id" | "path" | "package_manager">,
>(entries: readonly T[], query: string): T[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...entries];
  return entries.filter((entry) => {
    const pm = entry.package_manager?.toLowerCase() ?? "";
    return (
      entry.name.toLowerCase().includes(needle) ||
      entry.repo_id.toLowerCase().includes(needle) ||
      entry.path.toLowerCase().includes(needle) ||
      pm.includes(needle)
    );
  });
}

export function truncateWorkspacePath(rawPath: string, maxLength = 42): string {
  const trimmed = rawPath.trim();
  if (trimmed.length <= maxLength) return trimmed;
  if (maxLength <= 4) return `${trimmed.slice(0, Math.max(1, maxLength - 1))}…`;

  const sep = trimmed.includes("\\") && !trimmed.includes("/") ? "\\" : "/";
  const parts = trimmed.split(/[\\/]+/).filter(Boolean);
  if (parts.length <= 2) {
    const keepTail = Math.max(4, maxLength - 2);
    return `…${trimmed.slice(-keepTail)}`;
  }

  const hasLeadingSlash = trimmed.startsWith("/");
  const hasUncPrefix = trimmed.startsWith("\\\\");
  const rootPrefix = hasUncPrefix ? `\\\\${parts[0]}` : hasLeadingSlash ? `/${parts[0]}` : parts[0];
  const leaf = parts[parts.length - 1];
  const parent = parts[parts.length - 2];

  const withParent = `${rootPrefix}${sep}…${sep}${parent}${sep}${leaf}`;
  if (withParent.length <= maxLength) {
    return withParent;
  }

  const withRootAndLeaf = `${rootPrefix}${sep}…${sep}${leaf}`;
  if (withRootAndLeaf.length <= maxLength) {
    return withRootAndLeaf;
  }

  const tailOnly = `…${sep}${leaf}`;
  if (tailOnly.length <= maxLength) {
    return tailOnly;
  }

  return `…${leaf.slice(-(maxLength - 1))}`;
}

export function detectPreviewPackageManager(repoPath: string): string {
  const lower = repoPath.toLowerCase();
  if (lower.includes("rust") || lower.includes("cargo")) return "cargo";
  if (lower.includes("bun")) return "bun";
  if (lower.includes("pnpm")) return "pnpm";
  if (lower.includes("yarn")) return "yarn";
  if (lower.includes("npm") || lower.includes("node")) return "npm";
  if (
    lower.includes("py") ||
    lower.includes("uv") ||
    lower.includes("ml") ||
    lower.includes("torch")
  )
    return "uv";
  return "generic";
}
