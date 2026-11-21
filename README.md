# Relay: Context-Aware Cross-Device Coordination for Local AI Agents

COMP7608 Course Project — Tauri v2 + Rust Monorepo for cross-device local AI agent task handoff, deterministic Decision-Evidence DAG staleness propagation, `iroh` P2P sync, `rmcp` MCP integration, and isolated `git worktree` + `uv` self-healing.

## Repository Layout

```text
.
├── Cargo.toml                     # Rust Workspace (edition 2024, latest stable pinned crates)
├── docs/
│   ├── contracts.md               # Day 1 frozen contracts (MCP, DAG, TaskSpec, Wire RPC, ChaosCase)
│   └── api-cheatsheet.md          # Verified late-2026 Rust API patterns for AI-assisted coding
├── demo/                          # [Track 6] Dual-mode demo scripts & recordings
├── prompts/                       # [Track 3] Edge/Host prompts & dsh MCP configuration
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

```powershell
# Verify full Rust workspace compiles
cargo check --workspace

# Run Rust core/daemon tests (including DAG STALE propagation tests)
cargo test -p relay-core -p relay-daemon

# Export TypeScript bindings once crates/relay-desktop/src exists
cargo run -p relay-desktop --bin export_bindings -- crates/relay-desktop/src/bindings.ts
```

## Local LLM Setup (Ollama)

```powershell
ollama pull qwen3.5:4b
ollama pull qwen3.5:9b
```
