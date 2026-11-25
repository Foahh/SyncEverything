# SyncEverything — Product Roadmap

This roadmap tracks what has shipped in **SyncEverything v0.1** (powered by the Relay engine) and the upcoming milestones planned for open-source releases.

---

## Shipped in v0.1

- **Encrypted P2P Sync, Persistent Trusted Peers & Revocation (`iroh` 1.5 + `iroh-blobs`)**: Direct LAN discovery, NAT-traversing relay fallback, air-gapped `--lan-only` mode, single-machine `LocalDualSandbox` loopback testing, Git bundle and tracked working-tree diff transfer, BLAKE3-verified execution logs, offline SQLite outbox replay, persistent `trusted_peers` with stable node identity and exponential-backoff startup auto-reconnect, plus interactive **Trusted Devices** management (rename, reconnect, revoke, remove) enforced by Host `PEER_REVOKED` RPC gating, `RevocationGuardedBlobs`, and live QUIC disconnect.
- **Causal Decision-Evidence DAG & Deterministic `STALE` Quarantine**: SQLite WAL persistence, `petgraph` DAG engine, real background Git `HEAD` watching that records new `Artifact` nodes and cascades `Outdated` (`STALE`) status to downstream `Evidence`, plus deterministic MCP redaction and refusal of stale evidence reads.
- **Isolated `git worktree` Self-Healing, Live Streaming Logs & Cancellation**: Whitelisted `relay.toml` preset execution with parameter validation and timeouts, isolated per-task worktrees, live `stdout` and `stderr` for runs executed by the desktop process, persisted partial logs on the executor, and running/queued task cancellation (`TaskState::Cancelled`, local and remote `CancelTask` RPC). Failed runs provide ±10-line `error_snippets`. Agents can inspect files with confined `read_worktree_file` and propose `dependency_spec` or atomic `code_diff` patches with automatic rollback on smoke failure. Approval works through native MCP Elicitation or the desktop diff review.
- **Extensible `PackageManagerAdapter` Layer (L1/L2/L3)**: Priority-ordered adapter registry shipping built-in `uv`, `cargo`, `bun`, `pnpm`, `yarn`, `npm`, and `generic` adapters for starter `relay.toml` preset generation (L1), `<pm>:blake3:<16hex>` environment fingerprinting & lockfile dependency diffing (L2), and security-hardened `dependency_spec` self-healing (L3).
- **9-Tool Standard MCP Server (`rmcp` 3.11) & Safe Project Config Writer**: Stdio (`relay-daemon mcp-stdio`) and Streamable HTTP (`relay-daemon serve` / `mcp-http` at `/mcp`) with dual-mode snippets in Desktop Settings and a safe preview-before-write project config installer for Claude Code (`.mcp.json`) and Cursor (`.cursor/mcp.json`) that preserves sibling servers, merges atomically, and blocks symlink/junction escapes (Codex CLI copy-only).
- **Zero-Friction Repository Onboarding & Recent Workspaces Switcher**: CLI `relay-daemon init` (`--package-manager`, `--dry-run`, `--force`) and Desktop **Recent Workspaces** quick switcher (10-entry LRU with detected package manager badge, missing-path detection, and one-click `relay.toml` initialization).
- **Built-in SyncEverything Agent & Model Controls**: Collapsible right workbench rail (`Agent` / `Details`) with streaming Markdown, live tool-call traces, inline approval cards, per-repo chat persistence, and configurable local Ollama or OpenAI-compatible Cloud API endpoints with reasoning-effort controls.
- **Cross-Platform Desktop Workbench**: Tauri v2 + React 19 + shadcn/ui + React Flow UI with bilingual `en` / `zh-CN` localization and Light/Dark themes.

---

## OSS Roadmap (v0.2 and Beyond)

### 1. Deeper Agent & Editor Integration
- **Expanded MCP client onboarding**: Add guided onboarding for additional MCP clients (such as Windsurf and other editors) alongside the existing project-level `.mcp.json` / `.cursor/mcp.json` writer and copyable snippets.
- **Automatic workspace sync on commit/save**: Optional Git hook or filesystem watcher to push bundles and update DAG state automatically as developers commit in their editor.

### 2. Additional Runtime & Container Adapters
- **Additional runtime adapters**: Build on `PackageManagerAdapter` to support Poetry, Conda, Go, .NET, vcpkg, and Docker/OCI container sandboxes.
- **Untracked-file archive sync**: Complete untracked-file archive bundling alongside tracked working-tree diffs in P2P sync bundles.

### 3. Advanced Execution Observability & Control
- **Host resource telemetry**: Surface live Host CPU, RAM, and GPU utilization alongside elapsed runtime in the Inspector Drawer.
- **Parameterized ad-hoc overrides**: Safely allow temporary per-run overrides (for example, pytest `-k` filters or hyperparameter sweeps) bounded by explicit manifest rules and approval gates.

### 4. Always-On Production Polish
- **System tray & background auto-start**: Run `relay-daemon` quietly in the Windows/macOS tray or menu bar across laptop sleep/wake cycles.
- **Visual two-way conflict resolver**: Side-by-side diff UI when both laptop and workstation modify the same file offline.
- **Signed installers & auto-updates**: Distribute signed `.msi` (Windows), `.dmg` (macOS), and Linux packages with a guided first-run daemon setup flow.
