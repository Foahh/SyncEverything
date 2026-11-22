# Relay — Post-Course Productization Roadmap

This document captures the product features intentionally deferred beyond the COMP7608 course scope (Tracks 1–6) so that Relay can evolve from a verified course system into an everyday tool for developers working across a laptop and a compute workstation.

---

## Scope Split

### Included in COMP7608 Delivery (Tracks 1–3 Enhancements)
These lightweight capabilities are being built right alongside Tracks 1–3 because they both strengthen the course demo and immediately make Relay usable on arbitrary user repositories:
- **Open any local Git repository (`relay.toml`)** from the desktop workbench, with dynamic preset loading instead of hardcoding `forecast-bench`.
- **Real Git `HEAD` watcher**: background detection of real `git commit` changes that automatically records new `Artifact` nodes and cascades `Outdated` (`STALE`) status to downstream `Evidence` nodes without needing manual simulation buttons.
- **`relay-daemon` ↔ `relay-desktop` Pairing Ticket interop + Host task execution**: headless `relay-daemon serve` prints ready-to-paste pairing tickets, opens a pairing window, and executes incoming preflight / self-healing / approval workflows over `iroh`.
- **Desktop & Daemon LLM Settings**: configure Ollama (`qwen3.5:4b` / `9b`) or any OpenAI-compatible Cloud API directly from the desktop UI (or `RELAY_LLM_*` env vars) with live connectivity probing.

---

## Deferred Backlog (Post-Course Productization)

### 1. Zero-Config Onboarding & Multi-Project Management
- **Project switcher**: maintain a list of recent/pinned repositories in the sidebar and switch workspaces seamlessly.
- **Auto-generate `relay.toml`**: scan `pyproject.toml`, `package.json`, `Cargo.toml`, or `Makefile` when opening a repository that lacks `relay.toml` and propose safe default presets (smoke check, test suite, train/build commands) with one-click creation.

### 2. Zero-Friction Integration with Existing AI Coding Tools
- **One-click MCP installer**: automatically register `relay-daemon mcp-stdio` or `mcp-http` into user configs for Claude Code, Codex CLI, Cursor, Windsurf, and DeepSeek Harness (`dsh`).
- **Automatic workspace sync on commit/save**: optional Git hook or file watcher to push bundles and update DAG state silently whenever the developer commits in their IDE.

### 3. General-Purpose Execution & Real-Time Observability
- **Live streaming logs**: stream stdout/stderr line-by-line into the Inspector Drawer while a task runs on the workstation.
- **Task cancellation & resource telemetry**: stop running jobs cleanly and display host CPU/RAM/GPU utilization and elapsed time.
- **Multi-runtime adapters**: extend beyond `uv` (Python) to support Conda, Docker/OCI containers, Bun/Node, and Cargo/Rust sandboxes.
- **Parameterized ad-hoc runs**: safely allow temporary overrides (e.g., pytest filter `-k`, extra hyperparameters) within explicit approval boundaries.

### 4. Built-In Natural Language Workbench Prompt
- Add an inline command bar in the desktop app (backed by the configured LLM + Relay's own MCP tools) so users can ask questions or dispatch runs directly without switching to a separate CLI client.

### 5. Always-On Production Polish
- **System tray & auto-start**: run `relay-daemon` silently in the Windows/macOS menu bar or system tray with auto-reconnect across laptop sleep/wake cycles.
- **Persistent trusted devices & revocation**: remember paired peers across restarts, label devices, and revoke compromised or retired devices from Settings.
- **Visual two-way conflict resolver**: side-by-side diff UI when both laptop and workstation edit the same file offline.
- **Signed installers & auto-updates**: distribute signed `.msi` (Windows) and `.dmg` (macOS) bundles with built-in daemon setup wizard.
