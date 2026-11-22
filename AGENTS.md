# AGENTS.md — Relay Repository Guide for Humans & AI Coding Agents

Shared engineering rules, verification gates, and Conventional Commit standards for all contributors and AI coding agents working on **Relay**.

---

## 1. Authoritative Project Docs (Read First, Do Not Duplicate)

- [docs/relay_proposal_v2.md](docs/relay_proposal_v2.md) — Problem statement, system architecture, scope, and evaluation plan
- [docs/relay_plan_v2.md](docs/relay_plan_v2.md) — 6-track parallel ownership boundaries, sprint milestones, and dual-mode demo plan
- [docs/contracts.md](docs/contracts.md) — Frozen Day 1 interfaces (MCP 8 tools, DAG, TaskSpec, Wire RPC, ChaosCase, `relay.toml`)
- [docs/api-cheatsheet.md](docs/api-cheatsheet.md) — Verified late-2026 Rust crate versions and API patterns
- [docs/frontend-design-spec.md](docs/frontend-design-spec.md) — Frontend design system and product UX guidelines

Bundled repo agent skills live in `.agents/skills/`: `shadcn`, `tailwind-design-system`, `modern-web-guidance`, and `react-doctor`.

---

## 2. Hard Engineering Invariants (Non-Negotiable)

### Rust Backend (`crates/relay-core`, `crates/relay-daemon`, `crates/relay-desktop/src-tauri`)
- **Respect frozen contracts** in `crates/relay-core/src/contracts/*`; regenerate `crates/relay-desktop/src/bindings.ts` whenever commands, events, or contract types change.
- **NEVER add `git2` (libgit2) or local Rust LLM inference crates (`candle`, `llama-cpp-rs`).**
  - Git operations (`worktree`, `bundle`, `diff`, `rev-parse`, `apply`) MUST invoke system `git` asynchronously via `tokio::process::Command`.
  - Python env/run operations MUST invoke `uv` asynchronously via `tokio::process::Command`.
  - LLM inference MUST call OpenAI-compatible HTTP `/chat/completions` via `reqwest` (Ollama or Cloud APIs).
- **NEVER execute arbitrary shell strings on the Host.** Every task MUST resolve against whitelisted presets and `allowed_params` in `<repo>/relay.toml`.
- **Verified crate traps (`Cargo.toml`, Rust `edition = "2024"`, MSRV `1.94`)**:
  - `iroh 1.5.0` + `iroh-blobs 0.105.0`: single `Endpoint` serving `iroh_blobs::ALPN` + `RELAY_RPC_ALPN` (`b"relay/rpc/1"`). Use `NetworkProfile::LocalOnly` for offline tests.
  - `rmcp 3.11.0`: use `ContentBlock::text`, `ServerConfig` (not `ServerInfo`), `#[tool_handler(router = self.tool_router)]`, and `rmcp::elicit_safe!`. Keep `mcp-stdio` stdout pure JSON-RPC (logs to stderr only).
  - `sqlx 0.9.1`: `SqlSafeStr` requires `&'static str` SQL literals — never pass `format!(...)` strings to `sqlx::query` / `sqlx::query_as`.
  - `tauri-specta =2.0.0-rc.29` + `specta =2.0.0-rc.28` + `specta-typescript 0.0.15`: no BigInt types (`i64`/`u64`/`usize`/`serde_json::Value`) in exported structs — use `relay_core::contracts::JsonValue` and `u32`. On Windows, export bindings via `bun run gen:bindings` (the `export_bindings` bin target), not `cargo test -p relay-desktop --lib`.
  - Keep `async` futures `Send` across `.await` points (collect borrowed iterators before `.await`).

### Frontend (`crates/relay-desktop/src/`)
- **Runtime & Stack**: **Bun** (`bun.lock`, `bun run ...`) + React 19 + Vite 8 + TypeScript 7 (`tsgo`) + Tailwind CSS v4 + **shadcn/ui** (`base-mira` / `@base-ui/react`, `--preset b38miVIYq`) + `@xyflow/react` + `@tanstack/react-query` + `react-hook-form` + `i18next`/`react-i18next` + `oxlint` + `oxfmt`.
- **Zero hand-written Tauri `invoke("...")` calls**: all IPC goes through `src/lib/relayApi.ts` → auto-generated `src/bindings.ts` (never manually edit `src/bindings.ts`).
- **Type-safe Bilingual i18n**: `src/i18n/i18next.d.ts` checks keys against `en.json`, and `src/i18n/index.ts` enforces compile-time parity with `zh-CN.json`. Every UI label MUST exist in both files.
- **Product-First UX Language**: keep academic terms (`Evidence`, `Artifact`, `STALE`, `Elicitation`) in Rust code and papers; use everyday product copy in UI (`Result`, `Code`, `Outdated`, `Suggested Fix`). Keep dev/sandbox triggers tucked away from primary workspace chrome.
- **No machine-specific paths or secrets**: never commit absolute user paths, local tool installation paths, `.env*`, API keys, or local DB/runtime files.

---

## 3. Required Verification Gates (Run Before Every Commit)

Ensure **0 errors and 0 warnings**:

```powershell
# 1. Rust tests & full workspace check (from repo root)
cargo test -p relay-core -p relay-daemon
cargo check --workspace

# 2. Regenerate TypeScript bindings after src-tauri or contract changes
cd crates/relay-desktop
bun run gen:bindings

# 3. Frontend format, lint, typecheck & build (from crates/relay-desktop)
bun run format:check
bun run lint
bun run build
```

> Keep `crates/relay-desktop/dist/.gitkeep` tracked (`git checkout -- crates/relay-desktop/dist/.gitkeep` after `bun run build`) so fresh clones pass `cargo check --workspace`.

---

## 4. Git Workflow & Conventional Commits (Mandatory)

All commits MUST follow **[Conventional Commits 1.0.0](https://www.conventionalcommits.org/)**:

```text
<type>(<scope>): <concise imperative summary>
```

- **Types**: `feat` | `fix` | `refactor` | `docs` | `test` | `bench` | `perf` | `chore` | `ci`
- **Scopes**: `core` | `sandbox` | `sync` | `mcp` | `agent` | `daemon` | `desktop` | `i18n` | `contracts` | `bench` | `chaos` | `prompts` | `eval` | `docs`
- **Rules**: atomic per logical change, imperative lowercase summary ≤ 72 chars with no trailing period; use feature branches (`feat/track3-dsh-prompts`, `feat/track4-chaos-suite`) and PRs into `main`.
- **Never commit**: `.relay/`, `.relay-desktop/`, `*.db*`, `.env*`, `target/`, `node_modules/`, `crates/relay-desktop/dist/*` (except `.gitkeep`), `.venv/`, `.tmp/`.
