import { describe, expect, test } from "bun:test";
import { relayApi, resetBrowserPreviewState } from "./relayApi";
import {
  buildMcpSnippet,
  buildRelayMcpServerEntry,
  buildStdioArgs,
  computeJsonLineDiff,
  DEFAULT_MCP_HTTP_URL,
  getMcpClientConfigPath,
  mergeProjectMcpConfigJson,
  toProjectMcpClient,
  validateMcpHttpUrl,
  type McpWorkspaceTarget,
} from "./mcpConfig";

const SAMPLE_WORKSPACE: McpWorkspaceTarget = {
  repo_root: "/workspace/my-project",
  repo_id: "my-project",
  mcp_data_dir: "/home/dev/.local/share/relay-desktop/repos/my-project",
};

describe("mcpConfig stdio snippet generation", () => {
  test("builds Claude Code and Cursor JSON snippets with repo-root, repo-id, and data-dir", () => {
    const claudeRaw = buildMcpSnippet("claude", SAMPLE_WORKSPACE, { transport: "stdio" });
    const cursorRaw = buildMcpSnippet("cursor", SAMPLE_WORKSPACE, { transport: "stdio" });

    expect(claudeRaw).not.toBeNull();
    expect(cursorRaw).toBe(claudeRaw);

    const parsed = JSON.parse(claudeRaw!) as {
      mcpServers: { relay: { command: string; args: string[] } };
    };
    expect(parsed.mcpServers.relay.command).toBe("relay-daemon");
    expect(parsed.mcpServers.relay.args).toEqual([
      "--repo-root",
      "/workspace/my-project",
      "--repo-id",
      "my-project",
      "--data-dir",
      "/home/dev/.local/share/relay-desktop/repos/my-project",
      "mcp-stdio",
    ]);
  });

  test("returns null for stdio when mcp_data_dir is missing", () => {
    const args = buildStdioArgs({
      repo_root: "D:/repos/demo",
      repo_id: "demo",
      mcp_data_dir: null,
    });
    expect(args).toBeNull();
  });

  test("builds valid Codex CLI TOML snippet for stdio", () => {
    const codex = buildMcpSnippet("codex", SAMPLE_WORKSPACE, { transport: "stdio" });
    expect(codex).toBe(
      [
        "[mcp_servers.relay]",
        'command = "relay-daemon"',
        'args = ["--repo-root","/workspace/my-project","--repo-id","my-project","--data-dir","/home/dev/.local/share/relay-desktop/repos/my-project","mcp-stdio"]',
      ].join("\n"),
    );
    const parsed = Bun.TOML.parse(codex!) as {
      mcp_servers: { relay: { command: string; args: string[] } };
    };
    expect(parsed.mcp_servers.relay.command).toBe("relay-daemon");
    expect(parsed.mcp_servers.relay.args).toEqual(buildStdioArgs(SAMPLE_WORKSPACE)!);
  });

  test("returns null for stdio when no repository is open", () => {
    expect(buildMcpSnippet("claude", undefined, { transport: "stdio" })).toBeNull();
    expect(
      buildMcpSnippet("codex", { repo_root: null, repo_id: "", mcp_data_dir: null }),
    ).toBeNull();
  });
});

describe("mcpConfig streamable HTTP snippet generation", () => {
  test("uses default HTTP URL when omitted and works without an open repository", () => {
    const claude = JSON.parse(
      buildMcpSnippet("claude", undefined, { transport: "streamable_http" })!,
    );
    expect(claude).toEqual({
      mcpServers: {
        relay: {
          type: "http",
          url: DEFAULT_MCP_HTTP_URL,
        },
      },
    });

    const cursor = JSON.parse(
      buildMcpSnippet("cursor", undefined, { transport: "streamable_http" })!,
    );
    expect(cursor).toEqual({
      mcpServers: {
        relay: {
          url: DEFAULT_MCP_HTTP_URL,
        },
      },
    });

    const codex = buildMcpSnippet("codex", undefined, { transport: "streamable_http" });
    expect(codex).toBe(`[mcp_servers.relay]\nurl = "${DEFAULT_MCP_HTTP_URL}"`);
    const parsedCodex = Bun.TOML.parse(codex!) as {
      mcp_servers: { relay: { url: string } };
    };
    expect(parsedCodex.mcp_servers.relay.url).toBe(DEFAULT_MCP_HTTP_URL);
  });

  test("supports custom HTTP URLs and trims surrounding spaces", () => {
    const customUrl = "  https://gpu-host.internal:8443/mcp  ";
    const claude = JSON.parse(
      buildMcpSnippet("claude", SAMPLE_WORKSPACE, {
        transport: "streamable_http",
        httpUrl: customUrl,
      })!,
    );
    expect(claude.mcpServers.relay).toEqual({
      type: "http",
      url: "https://gpu-host.internal:8443/mcp",
    });

    const codex = buildMcpSnippet("codex", SAMPLE_WORKSPACE, {
      transport: "streamable_http",
      httpUrl: customUrl,
    });
    expect(codex).toBe('[mcp_servers.relay]\nurl = "https://gpu-host.internal:8443/mcp"');
  });

  test("rejects invalid HTTP URLs", () => {
    for (const invalid of [
      "",
      "   ",
      "127.0.0.1:7608/mcp",
      "file:///etc/passwd",
      "http://",
      "http://127.0.0.1:0/mcp",
      "http://user:secret@127.0.0.1:7608/mcp",
      "http://127.0.0.1:7608/mcp\n",
      "http://127.0.0.1:7608/mcp extra",
    ]) {
      expect(validateMcpHttpUrl(invalid).ok).toBe(false);
      expect(
        buildMcpSnippet("claude", SAMPLE_WORKSPACE, {
          transport: "streamable_http",
          httpUrl: invalid,
        }),
      ).toBeNull();
    }
  });
});

describe("project MCP target mapping and safe JSON merge", () => {
  test("restricts project file writing strictly to Claude Code and Cursor", () => {
    expect(toProjectMcpClient("claude")).toBe("claude_code");
    expect(toProjectMcpClient("cursor")).toBe("cursor");
    expect(toProjectMcpClient("codex")).toBeNull();

    expect(getMcpClientConfigPath("claude")).toBe(".mcp.json");
    expect(getMcpClientConfigPath("cursor")).toBe(".cursor/mcp.json");
    expect(getMcpClientConfigPath("codex")).toBe("~/.codex/config.toml");
  });

  test("creates new JSON when file does not exist and preserves siblings on merge", () => {
    const relayEntry = buildRelayMcpServerEntry("claude", SAMPLE_WORKSPACE, {
      transport: "streamable_http",
      httpUrl: "http://127.0.0.1:7608/mcp",
    })!;

    const created = mergeProjectMcpConfigJson(null, relayEntry, ".mcp.json");
    expect(created.status).toBe("create_new");
    expect(created.preservedServerNames).toEqual([]);
    expect(JSON.parse(created.mergedContent)).toEqual({
      mcpServers: { relay: relayEntry },
    });

    const existing = JSON.stringify(
      {
        $schema: "https://example.com/schema.json",
        mcpServers: {
          github: { command: "npx", args: ["-y", "github-mcp"] },
        },
      },
      null,
      2,
    );
    const merged = mergeProjectMcpConfigJson(existing, relayEntry, ".mcp.json");
    expect(merged.status).toBe("add_server");
    expect(merged.preservedServerNames).toEqual(["github"]);
    expect(JSON.parse(merged.mergedContent)).toEqual({
      $schema: "https://example.com/schema.json",
      mcpServers: {
        github: { command: "npx", args: ["-y", "github-mcp"] },
        relay: relayEntry,
      },
    });

    const unchanged = mergeProjectMcpConfigJson(merged.mergedContent, relayEntry, ".mcp.json");
    expect(unchanged.status).toBe("unchanged");

    const updatedEntry = buildRelayMcpServerEntry("claude", SAMPLE_WORKSPACE, {
      transport: "streamable_http",
      httpUrl: "http://127.0.0.1:9999/mcp",
    })!;
    const updated = mergeProjectMcpConfigJson(merged.mergedContent, updatedEntry, ".mcp.json");
    expect(updated.status).toBe("update_server");
    expect(updated.preservedServerNames).toEqual(["github"]);
  });

  test("refuses to overwrite corrupted JSON or non-object structures", () => {
    const relayEntry = { type: "http", url: DEFAULT_MCP_HTTP_URL };
    expect(() => mergeProjectMcpConfigJson("{broken", relayEntry, ".mcp.json")).toThrow(
      /invalid JSON/i,
    );
    expect(() => mergeProjectMcpConfigJson("[]", relayEntry, ".mcp.json")).toThrow(
      /root JSON value must be an object/i,
    );
    expect(() => mergeProjectMcpConfigJson('{"mcpServers": []}', relayEntry, ".mcp.json")).toThrow(
      /"mcpServers" must be a JSON object/i,
    );
  });

  test("computes line diffs between existing and merged JSON", () => {
    const diff = computeJsonLineDiff('{\n  "a": 1\n}\n', '{\n  "a": 2,\n  "b": 3\n}\n');
    expect(diff.some((line) => line.kind === "removed" && line.text.includes('"a": 1'))).toBe(true);
    expect(diff.some((line) => line.kind === "added" && line.text.includes('"a": 2,'))).toBe(true);
    expect(diff.some((line) => line.kind === "added" && line.text.includes('"b": 3'))).toBe(true);
  });
});

test("project writes require the confirmed target and file contents", async () => {
  resetBrowserPreviewState();
  const request = {
    client: "claude_code",
    transport: "streamable_http",
    http_url: DEFAULT_MCP_HTTP_URL,
  } as const;
  try {
    await relayApi.openWorkspaceRepo("/projects/first");
    const first = await relayApi.previewProjectMcpConfig(request);
    await relayApi.openWorkspaceRepo("/projects/second");
    await expect(relayApi.writeProjectMcpConfig(request, first)).rejects.toThrow(
      "Refresh the preview",
    );
    const second = await relayApi.previewProjectMcpConfig(request);
    expect(second.file_exists).toBe(false);
    await relayApi.writeProjectMcpConfig(request, second);
    await expect(relayApi.writeProjectMcpConfig(request, second)).rejects.toThrow(
      "Refresh the preview",
    );
  } finally {
    resetBrowserPreviewState();
  }
});
