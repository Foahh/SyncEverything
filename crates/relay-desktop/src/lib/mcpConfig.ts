import type {
  McpTransportMode,
  ProjectMcpClient,
  ProjectMcpFileStatus,
  WorkspaceInfo_Serialize,
} from "../bindings";

export const DEFAULT_MCP_HTTP_URL = "http://127.0.0.1:7608/mcp";

export type McpClientId = "claude" | "cursor" | "codex";

export type McpWorkspaceTarget = Pick<
  WorkspaceInfo_Serialize,
  "repo_root" | "mcp_data_dir" | "repo_id"
>;

export interface McpSnippetOptions {
  transport?: McpTransportMode;
  httpUrl?: string | null;
}

export type HttpUrlValidationResult = { ok: true; url: string } | { ok: false; error: string };

export interface MergedProjectMcpJson {
  mergedContent: string;
  status: ProjectMcpFileStatus;
  preservedServerNames: string[];
}

export interface McpDiffLine {
  kind: "context" | "added" | "removed";
  text: string;
}

const WHITESPACE_RE = /\s/;
const ALLOWED_HOST_RE = /^[A-Za-z0-9.:\-[\]]+$/;

function hasControlChar(value: string): boolean {
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    if (code <= 0x1f || code === 0x7f) {
      return true;
    }
  }
  return false;
}

export function toProjectMcpClient(client: McpClientId): ProjectMcpClient | null {
  if (client === "claude") return "claude_code";
  if (client === "cursor") return "cursor";
  return null;
}

export function getMcpClientConfigPath(client: McpClientId): string {
  switch (client) {
    case "claude":
      return ".mcp.json";
    case "cursor":
      return ".cursor/mcp.json";
    case "codex":
      return "~/.codex/config.toml";
  }
}

export function validateMcpHttpUrl(raw: string): HttpUrlValidationResult {
  if (hasControlChar(raw)) {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL cannot contain control characters.",
    };
  }
  const trimmed = raw.trim();
  if (!trimmed) {
    return { ok: false, error: "MCP HTTP endpoint URL cannot be empty." };
  }
  if (WHITESPACE_RE.test(trimmed)) {
    return { ok: false, error: "MCP HTTP endpoint URL cannot contain whitespace." };
  }
  if (!trimmed.startsWith("http://") && !trimmed.startsWith("https://")) {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL must start with http:// or https://.",
    };
  }
  const rest = trimmed.slice(trimmed.indexOf("://") + 3);
  const authority = rest.split(/[/?#]/, 1)[0] ?? "";
  if (!authority) {
    return { ok: false, error: "MCP HTTP endpoint URL must include a valid host." };
  }
  if (authority.includes("@")) {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL must not include embedded credentials.",
    };
  }

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return { ok: false, error: "MCP HTTP endpoint URL is not a valid URL." };
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL must start with http:// or https://.",
    };
  }
  if (parsed.username || parsed.password) {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL must not include embedded credentials.",
    };
  }
  if (!parsed.hostname || !ALLOWED_HOST_RE.test(parsed.hostname)) {
    return { ok: false, error: "MCP HTTP endpoint URL contains an invalid host." };
  }
  if (authority.endsWith(":0") || parsed.port === "0") {
    return {
      ok: false,
      error: "MCP HTTP endpoint URL port must be between 1 and 65535.",
    };
  }

  return { ok: true, url: trimmed };
}

export function buildStdioArgs(workspace?: McpWorkspaceTarget): string[] | null {
  if (!workspace?.repo_root || !workspace.mcp_data_dir) return null;

  return [
    "--repo-root",
    workspace.repo_root,
    "--repo-id",
    workspace.repo_id,
    "--data-dir",
    workspace.mcp_data_dir,
    "mcp-stdio",
  ];
}

export function buildRelayMcpServerEntry(
  client: McpClientId,
  workspace?: McpWorkspaceTarget,
  options?: McpSnippetOptions,
): Record<string, unknown> | null {
  const transport: McpTransportMode = options?.transport ?? "stdio";
  if (transport === "stdio") {
    const args = buildStdioArgs(workspace);
    if (!args) return null;
    return {
      command: "relay-daemon",
      args,
    };
  }

  const rawUrl =
    options?.httpUrl === undefined || options?.httpUrl === null
      ? DEFAULT_MCP_HTTP_URL
      : options.httpUrl;
  const validated = validateMcpHttpUrl(rawUrl);
  if (!validated.ok) return null;

  if (client === "claude") {
    return {
      type: "http",
      url: validated.url,
    };
  }
  return {
    url: validated.url,
  };
}

export function buildMcpSnippet(
  client: McpClientId,
  workspace?: McpWorkspaceTarget,
  options?: McpSnippetOptions,
): string | null {
  const transport: McpTransportMode = options?.transport ?? "stdio";
  const entry = buildRelayMcpServerEntry(client, workspace, {
    transport,
    httpUrl: options?.httpUrl,
  });
  if (!entry) return null;

  if (client === "codex") {
    if (transport === "streamable_http") {
      return ["[mcp_servers.relay]", `url = ${JSON.stringify(entry.url)}`].join("\n");
    }
    return [
      "[mcp_servers.relay]",
      'command = "relay-daemon"',
      `args = ${JSON.stringify(entry.args)}`,
    ].join("\n");
  }

  return JSON.stringify({ mcpServers: { relay: entry } }, null, 2);
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stableNormalize(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(stableNormalize);
  }
  if (isPlainObject(value)) {
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(value).sort()) {
      sorted[key] = stableNormalize(value[key]);
    }
    return sorted;
  }
  return value;
}

export function mergeProjectMcpConfigJson(
  existingRaw: string | null,
  relayEntry: Record<string, unknown>,
  relativePath: string,
): MergedProjectMcpJson {
  let root: Record<string, unknown>;
  const existed = existingRaw !== null;

  if (!existed) {
    root = {};
  } else {
    let parsed: unknown;
    try {
      parsed = JSON.parse(existingRaw);
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      throw new Error(
        `Refusing to overwrite ${relativePath}: existing file contains invalid JSON (${detail}). Fix or remove the file before writing.`,
      );
    }
    if (!isPlainObject(parsed)) {
      throw new Error(`Refusing to overwrite ${relativePath}: root JSON value must be an object.`);
    }
    root = { ...parsed };
  }

  let status: ProjectMcpFileStatus;
  let preservedServerNames: string[] = [];

  if (!Object.hasOwn(root, "mcpServers")) {
    root.mcpServers = { relay: relayEntry };
    status = existed ? "add_server" : "create_new";
  } else {
    const rawServers = root.mcpServers;
    if (!isPlainObject(rawServers)) {
      throw new Error(`Refusing to overwrite ${relativePath}: "mcpServers" must be a JSON object.`);
    }
    const nextServers: Record<string, unknown> = { ...rawServers };
    preservedServerNames = Object.keys(nextServers)
      .filter((key) => key !== "relay")
      .sort();

    if (!Object.hasOwn(nextServers, "relay")) {
      status = "add_server";
    } else if (
      JSON.stringify(stableNormalize(nextServers.relay)) ===
      JSON.stringify(stableNormalize(relayEntry))
    ) {
      status = "unchanged";
    } else {
      status = "update_server";
    }

    nextServers.relay = relayEntry;
    root.mcpServers = nextServers;
  }

  return {
    mergedContent: `${JSON.stringify(root, null, 2)}\n`,
    status,
    preservedServerNames,
  };
}

export function computeJsonLineDiff(
  existingRaw: string | null,
  mergedContent: string,
): McpDiffLine[] {
  const newLines = mergedContent.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n");
  if (existingRaw === null) {
    return newLines.map((text) => ({ kind: "added", text }));
  }

  const oldLines = existingRaw.replace(/\r\n/g, "\n").replace(/\n$/, "").split("\n");
  const m = oldLines.length;
  const n = newLines.length;
  const dp: number[][] = Array.from({ length: m + 1 }, () => new Array<number>(n + 1).fill(0));

  for (let i = m - 1; i >= 0; i -= 1) {
    for (let j = n - 1; j >= 0; j -= 1) {
      if (oldLines[i] === newLines[j]) {
        dp[i][j] = dp[i + 1][j + 1] + 1;
      } else {
        dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1]);
      }
    }
  }

  const diff: McpDiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < m && j < n) {
    if (oldLines[i] === newLines[j]) {
      diff.push({ kind: "context", text: oldLines[i] });
      i += 1;
      j += 1;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      diff.push({ kind: "removed", text: oldLines[i] });
      i += 1;
    } else {
      diff.push({ kind: "added", text: newLines[j] });
      j += 1;
    }
  }
  while (i < m) {
    diff.push({ kind: "removed", text: oldLines[i] });
    i += 1;
  }
  while (j < n) {
    diff.push({ kind: "added", text: newLines[j] });
    j += 1;
  }

  return diff;
}
