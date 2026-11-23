# Relay: Context-Aware Cross-Device Coordination for Local AI Agents

COMP7608 Course Project — Tauri v2 + Rust Monorepo for cross-device local AI agent task handoff, deterministic Decision-Evidence DAG staleness propagation, `iroh` P2P sync, `rmcp` MCP integration, and isolated `git worktree` + `uv` self-healing.

## Repository Layout

```text
.
├── AGENTS.md                      # Contributor & AI Agent rules, verification gates & Conventional Commits
├── Cargo.toml                     # Rust Workspace (edition 2024, latest stable pinned crates)
├── docs/
│   ├── relay_proposal.md          # Project proposal, architecture & evaluation plan
│   ├── relay_plan.md              # 6-track parallel ownership, sprints & demo plan
│   ├── contracts.md               # Day 1 frozen contracts (MCP, DAG, TaskSpec, Wire RPC, ChaosCase)
│   ├── api-cheatsheet.md          # Verified late-2026 Rust API patterns for AI-assisted coding
│   └── frontend-design-spec.md    # Frontend design system & product UX guidelines
├── .agents/skills/                # Bundled AI coding skills (shadcn, tailwind-v4, modern-web-guidance, react-doctor)
├── demo/                          # [Track 6] Dual-mode demo scripts & recordings
├── prompts/                       # [Track 3] Edge/Host prompts & MCP client configurations
├── eval/models/                   # [Track 3] Local Ollama (qwen3.5:4b/9b) vs. Cloud API benchmarks
├── bench/
│   ├── fixture-repo/              # [Track 4] Reproducible forecasting experiment repo with relay.toml
│   ├── chaos-cases/               # [Track 4] Fault-injection Chaos Suite JSON specs & runners
│   └── results/                   # [Track 5] Raw benchmark logs & plotting scripts
└── crates/
    ├── relay-core/                # [Track 1] Contracts, DAG engine, SQLite WAL, iroh, rmcp, git/uv sandbox, Agent loop
    ├── relay-daemon/              # [Track 1] Headless Host/Client daemon & MCP stdio/HTTP CLI
    └── relay-desktop/             # [Track 1/2] Tauri v2 desktop app (src-tauri + frontend via create app)
```

## Quick Verification

> Contributors & AI agents: read [AGENTS.md](AGENTS.md) before opening a PR (covers 6-track ownership, Rust/Tauri/shadcn invariants, and mandatory Conventional Commits).

```powershell
# Verify full Rust workspace compiles
cargo check --workspace

# Run Rust core/daemon tests (including DAG STALE, worktree self-healing, iroh P2P & MCP tests)
cargo test -p relay-core -p relay-daemon

# Install desktop frontend dependencies (Bun)
cd crates/relay-desktop
bun install

# Export TypeScript bindings after modifying src-tauri commands or contracts
bun run gen:bindings

# Run frontend format check, lint & build
bun run format:check && bun run lint && bun run build

# Launch desktop workbench (defaults to LocalDualSandbox)
bun run tauri dev
```

## Local LLM Setup (Ollama)

```powershell
ollama pull qwen3.5:4b
ollama pull qwen3.5:9b
```
