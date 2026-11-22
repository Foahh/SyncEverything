# AGENTS.md — Relay Repository Guide for Humans & AI Coding Agents

This file defines architecture boundaries, verified late-2026 dependency rules, validation gates, and Conventional Commit standards for all contributors and AI coding agents (Codex CLI, Claude Code, Antigravity, Cursor, DeepSeek Harness) working on **Relay**.

---

## 1. Repository Map & Parallel Track Ownership

Respect directory boundaries to avoid merge conflicts across the 6-person team:

| Track | Scope | Primary Directories |
|---|---|---|
| **Track 1** | Core Rust engine, `iroh` P2P, SQLite WAL, `git worktree`/`uv` sandbox, `rmcp` MCP server, Tauri backend | `crates/relay-core/`, `crates/relay-daemon/`, `crates/relay-desktop/src-tauri/` |
| **Track 2** | Tauri desktop frontend (React 19 + HeroUI v3 + React Flow + EN/ZH i18n) | `crates/relay-desktop/src/` |
| **Track 3** | Prompt engineering, `dsh` MCP integration, Ollama (`qwen3.5:4b/9b`) vs. Cloud API benchmarks | `prompts/`, `eval/models/` |
| **Track 4** | Forecasting fixture repo & 8-case Chaos fault-injection suite | `bench/fixture-repo/`, `bench/chaos-cases/` |
| **Track 5** | Baseline/ablation experiment runs, raw logs, and publication plots | `bench/results/` |
| **Track 6** | Windows + macOS QA, dual-mode live demo assets, final report & slides | `docs/`, `demo/` |

Before making cross-track changes, read:
- [docs/contracts.md](docs/contracts.md) — Frozen Day 1 interfaces (MCP 8 tools, DAG, TaskSpec, Wire RPC, ChaosCase, `relay.toml`)
- [docs/api-cheatsheet.md](docs/api-cheatsheet.md) — Verified Rust crate versions and API patterns
- [docs/frontend-design-spec.md](docs/frontend-design-spec.md) — Frontend design system and product UX guidelines

---

## 2. Hard Engineering Invariants (Non-Negotiable)

### Rust Backend (`crates/relay-core`, `crates/relay-daemon`, `crates/relay-desktop/src-tauri`)
- **Do NOT break frozen contracts** in `crates/relay-core/src/contracts/*` without coordinating across tracks and regenerating `crates/relay-desktop/src/bindings.ts`.
- **NEVER add `git2` (libgit2) or local Rust LLM inference crates (`candle`, `llama-cpp-rs`).**
  - Git operations (`worktree`, `bundle`, `diff`, `rev-parse`, `apply`) MUST invoke system `git` asynchronously via `tokio::process::Command`.
  - Python env/run operations MUST invoke `uv` asynchronously via `tokio::process::Command`.
  - LLM inference MUST call OpenAI-compatible HTTP `/chat/completions` endpoints via `reqwest` (works identically with Ollama and Cloud APIs).
- **NEVER execute arbitrary shell strings on the Host.** Every runnable task MUST resolve against whitelisted presets and `allowed_params` in `<repo>/relay.toml`.
- **Verified crate traps (late-2026 pins in `Cargo.toml`, Rust `edition = "2024"`, MSRV `1.94`)**:
  - `iroh = "1.5.0"` + `iroh-blobs = "0.105.0"`: single `Endpoint` serving both `iroh_blobs::ALPN` and `RELAY_RPC_ALPN` (`b"relay/rpc/1"`). Use `NetworkProfile::LocalOnly` for offline/loopback tests.
  - `rmcp = "3.11.0"`: use `ContentBlock::text`, `ServerConfig` (not deprecated `ServerInfo`), `#[tool_handler(router = self.tool_router)]`, and `rmcp::elicit_safe!`. Keep stdout pure JSON-RPC in `mcp-stdio` (logs to stderr only).
  - `sqlx = "0.9.1"`: `SqlSafeStr` requires `&'static str` SQL literals — never pass `format!(...)` strings to `sqlx::query` / `sqlx::query_as`.
  - `tauri-specta = "=2.0.0-rc.29"` + `specta = "=2.0.0-rc.28"` + `specta-typescript = "0.0.15"`:
    - Do NOT put `i64`/`u64`/`usize` or `serde_json::Value` in `#[derive(specta::Type)]` structs; use `relay_core::contracts::JsonValue` (`f64`-backed) and `u32`.
    - On Windows, export TypeScript bindings via the `[[bin]]` target (`cargo run -p relay-desktop --bin export_bindings -- crates/relay-desktop/src/bindings.ts`), NOT `cargo test -p relay-desktop --lib` (which lacks the Tauri ComCtl32 v6 manifest).
  - Keep `async` futures `Send` across `.await` points (e.g., collect borrowed iterator results before `.await`).

### Frontend (`crates/relay-desktop/src/`)
- **Stack**: React 19 + Vite 8 + TypeScript 7 (`tsgo`) + Tailwind CSS v4 + HeroUI v3 (`@heroui/react 3.2.7` compound API + `@heroui/styles`) + `@xyflow/react` + `@tanstack/react-query` + `react-hook-form` + `i18next`/`react-i18next` + `oxlint` + `oxfmt`.
- **Zero hand-written Tauri `invoke("...")` calls**: all IPC goes through `src/lib/relayApi.ts` → auto-generated `src/bindings.ts` (never manually edit `src/bindings.ts`).
- **Type-safe Bilingual i18n**: `src/i18n/i18next.d.ts` enforces strict key checking against `en.json`, and `src/i18n/index.ts` enforces compile-time parity for `zh-CN.json`. Every new UI label MUST be added to both locale files.
- **Product-First UX Language**: keep academic/system terms (`Evidence`, `Artifact`, `STALE`, `Elicitation`) in Rust code and papers, while using natural user-facing copy in UI (`Result`, `Code`, `Outdated`, `Suggested Fix`). Keep dev/sandbox triggers tucked away from primary workspace chrome.

---

## 3. Required Verification Gates (Run Before Every Commit)

Run the relevant checks and ensure **0 errors and 0 warnings**:

```powershell
# 1. Rust core + daemon tests & full workspace check (from repo root)
cargo test -p relay-core -p relay-daemon
cargo check --workspace

# 2. Regenerate TypeScript bindings whenever src-tauri commands/events or contracts change
cd crates/relay-desktop
pnpm gen:bindings

# 3. Frontend format, lint, typecheck & production build (from crates/relay-desktop)
pnpm format:check
pnpm lint
pnpm build
```

> **Note for `crates/relay-desktop/dist/.gitkeep`**: `vite build` clears `dist/`. Keep `crates/relay-desktop/dist/.gitkeep` tracked (run `git checkout -- crates/relay-desktop/dist/.gitkeep` after `pnpm build`) so fresh clones pass `cargo check --workspace` immediately.

---

## 4. Git Workflow & Conventional Commits (Mandatory)

All commits on `main` and feature branches MUST follow **[Conventional Commits 1.0.0](https://www.conventionalcommits.org/)**:

```text
<type>(<scope>): <concise imperative summary>

[optional body explaining WHY the change was made]

[optional footer(s), e.g., BREAKING CHANGE: ...]
```

### Allowed Types
- `feat` — New user-facing feature, crate capability, MCP tool, or benchmark case
- `fix` — Bug fix, race condition fix, or regression fix
- `refactor` — Code or UI restructuring without changing core semantics
- `docs` — Documentation, specs, reports, slides, `AGENTS.md`, `README.md`
- `test` — Adding or expanding unit/integration/Chaos tests
- `bench` — Benchmark harnesses, fixture updates, evaluation scripts, plots
- `perf` — Performance or latency optimization
- `chore` — Tooling, dependency updates, workspace scaffolding, `.gitignore`
- `ci` — GitHub Actions / CI pipelines

### Standard Scopes
`core` | `sandbox` | `sync` | `mcp` | `agent` | `daemon` | `desktop` | `i18n` | `contracts` | `bench` | `chaos` | `prompts` | `eval` | `docs`

### Commit Hygiene Rules
1. **Atomic commits**: one logical change per commit (do not mix unrelated refactors, feature code, and formatting noise).
2. **Imperative lowercase summary**, ≤ 72 chars, no trailing period:
   - Good: `feat(chaos): add case_02 upstream code edit stale-evidence benchmark`
   - Good: `fix(sync): retry iroh-blobs fetch after transient relay switch`
   - Good: `docs(prompts): tune qwen3.5:9b host self-healing system prompt`
   - Bad: `update stuff`, `WIP`, `Fix bug.`
3. **Branching**: create short-lived branches per task (`feat/track3-dsh-prompts`, `feat/track4-chaos-suite`, `fix/desktop-pair-modal`) and open PRs into `main`.
4. **Never commit**:
   - Local runtime state: `.relay/`, `.relay-desktop/`, `*.db`, `*.db-wal`, `*.db-shm`
   - Secrets or credentials: `.env`, `.env.*`, API keys, private tokens
   - Build/cache outputs: `target/`, `node_modules/`, `crates/relay-desktop/dist/*` (except `.gitkeep`), `.venv/`, `.tmp/`
