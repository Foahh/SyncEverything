# Prompts & Standard MCP Client Setup (Track 3)

Relay exposes its 8 frozen tools over the **Model Context Protocol (`rmcp` 3.11)** via two standard transports:

- **Stdio (`relay-daemon mcp-stdio`)**: Best for local AI coding agents (Claude Code, Cursor, Codex CLI, Windsurf, OpenCode, or generic `dsh`) operating directly on a workstation or laptop checkout.
- **Streamable HTTP (`relay-daemon serve` / `relay-daemon mcp-http` at `http://127.0.0.1:7608/mcp`)**: Best when multiple clients or a remote laptop agent connect to a long-running Host daemon.

> All templates in `mcp-clients/` use portable placeholders (`<RELAY_DAEMON_BIN>`, `<REPO_ROOT>`, `<DATA_DIR>`) and contain no machine-specific paths or hardcoded third-party client prerelease versions.

---

## Files in This Directory

| File | Purpose |
|---|---|
| `edge-fast-path.md` | System prompt for laptop / edge quick status queries, note capture, and `STALE` quarantine |
| `host-deep-path.md` | System prompt for workstation preflight diagnosis, worktree self-healing, and human-in-the-loop execution |
| `routing-policy.md` | Decision policy choosing between Edge Fast-Path, Host Local Ollama (`qwen3.5:4b` / `9b`), and Cloud OpenAI-compatible APIs |
| `mcp-clients/mcp-stdio.json` | Standard `mcpServers` JSON config (Claude Code, Cursor, Windsurf, OpenCode) |
| `mcp-clients/mcp-http.json` | Standard Streamable HTTP `/mcp` config for a running Host daemon |
| `mcp-clients/codex.toml` | Codex CLI `[mcp_servers.relay]` TOML snippet |
| `mcp-clients/dsh.json` | Optional version-agnostic DeepSeek Harness MCP config |

---

## Quick Start: Connect Any MCP Agent

1. Build the daemon once:
   ```bash
   cargo build --release -p relay-daemon
   ```
2. Pick the matching template in `prompts/mcp-clients/` and replace `<RELAY_DAEMON_BIN>`, `<REPO_ROOT>`, and `<DATA_DIR>` with paths for your repository.
3. Optional: attach `edge-fast-path.md` or `host-deep-path.md` to your agent's custom instructions (note that `RelayMcpServer` also advertises its core anti-hallucination rules automatically via MCP server `instructions` during `initialize`).
4. Verify your agent sees all 8 frozen tools:
   `inspect_sync_status`, `list_handoffs`, `check_env_diff`, `get_dag_snapshot`, `read_evidence_log`, `run_preflight_in_worktree`, `propose_worktree_patch`, `approve_and_execute_task`.
