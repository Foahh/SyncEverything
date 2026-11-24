import type { WorkspaceInfo_Serialize } from "../bindings";

export type McpClientId = "claude" | "cursor" | "codex";

export function buildMcpSnippet(
  client: McpClientId,
  workspace?: Pick<WorkspaceInfo_Serialize, "repo_root" | "mcp_data_dir" | "repo_id">,
): string | null {
  if (!workspace?.repo_root || !workspace.mcp_data_dir) return null;
  const args = [
    "--repo-root",
    workspace.repo_root,
    "--repo-id",
    workspace.repo_id,
    "--data-dir",
    workspace.mcp_data_dir,
    "mcp-stdio",
  ];
  if (client === "codex") {
    return [
      "[mcp_servers.relay]",
      'command = "relay-daemon"',
      `args = ${JSON.stringify(args)}`,
    ].join("\n");
  }
  return JSON.stringify({ mcpServers: { relay: { command: "relay-daemon", args } } }, null, 2);
}
