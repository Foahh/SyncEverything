# Contributing to SyncEverything

Thank you for your interest in contributing to **SyncEverything** (powered by the Relay Rust engine)!

## Prerequisites

- **Rust** `1.94+` (edition 2024) and **Git**
- **Bun** `1.3+` (for `crates/relay-desktop`)
- **`uv`** and **Python `3.11+`** (for Python sandbox tests and evaluation scripts)
- **Tauri platform dependencies** (MSVC + WebView2 on Windows, Xcode Command Line Tools on macOS, WebKitGTK + GTK3 dev libraries on Linux — see [docs/development.md](docs/development.md#prerequisites))

## Engineering Conventions

Read [AGENTS.md](AGENTS.md) and [docs/architecture.md](docs/architecture.md) before opening a pull request. Key principles:

- **Greenfield policy**: Do not introduce backward-compatibility shims, legacy fallbacks, or deprecated aliases. Update callers and tests cleanly when contracts change.
- **Security invariants**: Host task execution must always resolve a whitelisted preset in `relay.toml` and validate parameters against `allowed_params`. Never execute arbitrary shell strings.
- **Runtime boundaries**: Invoke system `git` and `uv` via `tokio::process::Command` and LLM endpoints via `reqwest` (`/chat/completions`). Do not add `git2` or local Rust inference crates.
- **Typed Tauri IPC**: Never hand-edit `crates/relay-desktop/src/bindings.ts` or write manual `invoke("...")` calls. Whenever Tauri commands, events, or exported contract types change, run `bun run gen:bindings` from `crates/relay-desktop`.
- **Bilingual desktop copy**: Add user-facing UI strings to both `crates/relay-desktop/src/i18n/locales/en.json` and `crates/relay-desktop/src/i18n/locales/zh-CN.json`.

## Contributor Workflow

1. Fork the repository and create a short-lived branch from `main` (for example, `feat/cargo-adapter` or `fix/desktop-pairing`).
2. Keep each commit focused on a single logical change.
3. Run all verification checks for the areas you touched (see below).
4. Open a pull request against `main` describing the motivation, changes, and verification performed.

## Verification Commands

Run the checks relevant to your change and ensure zero warnings or errors:

```bash
# Rust tests and full workspace compilation (from repository root)
cargo test -p relay-core -p relay-daemon
cargo check --workspace

# Desktop frontend checks (from crates/relay-desktop)
bun install --frozen-lockfile
bun run format:check
bun run lint
bun test
bun run build

# Restore tracked dist placeholder after Vite build (from repository root)
git restore --worktree -- crates/relay-desktop/dist/.gitkeep

# Evaluation harness unit tests (from repository root)
uv run python -m unittest discover -s course/eval -p "test_*.py"
uv run python -m unittest discover -s course/bench/results -p "test_*.py"

# Whitespace and diff sanity check
git diff --check
```

## Conventional Commits

Follow the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) format with an imperative, lowercase header of at most 72 characters and no trailing period:

```text
<type>(<scope>): <summary>
```

- **Types**: `feat`, `fix`, `refactor`, `docs`, `test`, `bench`, `perf`, `chore`, `ci`
- **Scopes**: `core`, `sandbox`, `sync`, `mcp`, `agent`, `daemon`, `desktop`, `i18n`, `contracts`, `bench`, `chaos`, `prompts`, `eval`, `oss`, `docs`

## Pull Request Checklist

- [ ] All relevant Rust, frontend, and Python checks pass with zero warnings.
- [ ] `bun run gen:bindings` was re-run if any Tauri command, event, or exported contract changed.
- [ ] Both `en.json` and `zh-CN.json` were updated if any desktop UI copy changed.
- [ ] `crates/relay-desktop/dist/.gitkeep` remains tracked and clean (no Vite build output committed).
- [ ] No runtime state (`.relay/`, `*.db`), secrets (`.env`), or machine-specific paths are included in the diff.
