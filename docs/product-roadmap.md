# SyncEverything — Product Roadmap

This roadmap tracks what has shipped in **SyncEverything v0.1** (powered by the Relay engine) and the upcoming milestones planned for open-source releases.

---

## Shipped in v0.1

- **Encrypted P2P Sync & Multi-Mode Runtime (`iroh` 1.5 + `iroh-blobs`)**: Direct LAN discovery, NAT-traversing relay fallback, air-gapped `--lan-only` mode, single-machine `LocalDualSandbox` loopback testing, Git bundle and tracked working-tree diff transfer, BLAKE3-verified execution logs, and offline SQLite outbox replay.
- **Causal Decision-Evidence DAG & Deterministic `STALE` Quarantine**: SQLite WAL persistence, `petgraph` DAG engine, real background Git `HEAD` watching that records new `Artifact` nodes and cascades `Outdated` (`STALE`) status to downstream `Evidence`, plus deterministic MCP redaction and refusal of stale evidence reads.
- **Isolated `git worktree` Self-Healing & Approval Gates**: Whitelisted `relay.toml` preset execution with parameter validation and timeouts, isolated per-task worktrees, ±10-line `error_snippets`, confined `read_worktree_file` inspection, `propose_worktree_patch` supporting `dependency_spec` and atomic `code_diff` with automatic rollback on smoke failure, and dual-mode approval (native MCP Elicitation and SyncEverything Desktop Diff Approval).
- **9-Tool Standard MCP Server (`rmcp` 3.11)**: Stdio (`relay-daemon mcp-stdio`) and Streamable HTTP (`relay-daemon serve` / `mcp-http` at `/mcp`) with ready-to-paste client snippets for Claude Code, Cursor, and Codex CLI directly in Desktop Settings.
- **Zero-Friction Repository Onboarding**: Open any local Git repository from SyncEverything Desktop and auto-generate a starter `relay.toml` for Python (`uv`), Rust (`cargo`), JavaScript/TypeScript (`bun`/`pnpm`/`yarn`/`npm`), or generic Git projects.
- **Built-in SyncEverything Agent & Model Controls**: Collapsible right workbench rail (`Agent` / `Details`) with streaming Markdown, live tool-call traces, inline approval cards, per-repo chat persistence, and configurable local Ollama or OpenAI-compatible Cloud API endpoints with reasoning-effort controls.
- **Cross-Platform Desktop Workbench**: Tauri v2 + React 19 + shadcn/ui + React Flow UI with bilingual `en` / `zh-CN` localization and Light/Dark themes.

---

## OSS Roadmap (v0.2 and Beyond)

### 1. Multi-Project Workspace & Agent Integration
- **Project switcher**: Pin and switch between recent local repositories directly from the sidebar without re-browsing directories.
- **One-click MCP installer**: Optionally write `relay-daemon` server entries directly into user configuration files for Claude Code, Codex CLI, Cursor, Windsurf, and compatible clients (building on the existing copyable snippets).
- **Automatic workspace sync on commit/save**: Optional Git hook or filesystem watcher to push bundles and update DAG state automatically as developers commit in their editor.

### 2. Extensible Package Manager & Runtime Adapter Layer
- **Multi-runtime adapters**: Extend environment fingerprinting and automated dependency self-healing beyond `uv` (Python) to Conda, Docker/OCI containers, Bun/Node, and Cargo/Rust sandboxes behind a unified adapter trait.
- **Untracked-file archive sync**: Complete untracked-file archive bundling alongside tracked working-tree diffs in P2P sync bundles.

### 3. Real-Time Execution Observability & Control
- **Live streaming logs**: Stream stdout/stderr line-by-line into the Inspector Drawer while a task runs on the workstation.
- **Task cancellation & resource telemetry**: Stop running jobs cleanly and surface Host CPU, RAM, and GPU utilization alongside elapsed runtime.
- **Parameterized ad-hoc overrides**: Safely allow temporary per-run overrides (for example, pytest `-k` filters or hyperparameter sweeps) bounded by explicit manifest rules and approval gates.

### 4. Always-On Production Polish
- **System tray & background auto-start**: Run `relay-daemon` quietly in the Windows/macOS tray or menu bar with automatic reconnection across laptop sleep/wake cycles.
- **Persistent trusted peers & device revocation UI**: Label paired devices, remember trust across restarts, and revoke retired or compromised peers from Desktop Settings.
- **Visual two-way conflict resolver**: Side-by-side diff UI when both laptop and workstation modify the same file offline.
- **Signed installers & auto-updates**: Distribute signed `.msi` (Windows), `.dmg` (macOS), and Linux packages with a guided first-run daemon setup flow.
