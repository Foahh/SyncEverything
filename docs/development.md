# SyncEverything Development Guide

Use this guide to launch the SyncEverything workbench, run the `relay-daemon` locally, and verify a change. See the [architecture guide](architecture.md) to find the component that owns a behavior.

## Prerequisites

| Work | Required tools |
| --- | --- |
| Rust workspace | Git and a Rust toolchain compatible with the workspace. [`Cargo.toml`](../Cargo.toml) declares Rust 1.94 as the workspace minimum and uses edition 2024. Use [`Cargo.lock`](../Cargo.lock) for resolved dependency versions. |
| Desktop frontend | [Bun](https://bun.sh) (`1.3+`). Use [`package.json`](../crates/relay-desktop/package.json) for available scripts and [`bun.lock`](../crates/relay-desktop/bun.lock) for resolved dependencies. |
| Native desktop and binding export | Windows requires the MSVC Rust toolchain, Microsoft C++ Build Tools, and WebView2. On macOS, install the Xcode Command Line Tools. On Linux, install the WebKitGTK and GTK3 development packages. See [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). |
| Forecast fixture and worktree preflight | System Git and `uv` on `PATH`, plus Python 3.11 or newer as specified by the [fixture](../course/bench/fixture-repo/pyproject.toml). |
| Local model inference | Ollama running on `127.0.0.1:11434`. The desktop diagnosis path uses `qwen3.5:4b` by default. The [model evaluation suite](../course/eval/models/) also benchmarks `qwen3.5:9b`. |

Use an ignored directory such as `.tmp/` for scratch files and `.relay/` for runtime state.

## Start the desktop

From the repository root, install the locked frontend dependencies and launch Tauri.

```powershell
cd crates/relay-desktop
bun install --frozen-lockfile
bun run tauri dev
```

On a fresh desktop data directory, SyncEverything starts in `LocalDualSandbox`. The selected mode persists across restarts. Set `RELAY_DATA_DIR` before launch to use separate desktop state for a test or demo.

For browser-only UI work, run `bun run dev` from `crates/relay-desktop` and open `http://localhost:1420`. The browser uses in-memory preview data. Use Tauri for checks that exercise native IPC, storage, or networking.

## Prepare a repository for task execution

Open a Git checkout with at least one commit. If it has no `relay.toml`, SyncEverything Desktop can generate a starter manifest for its project type (`python-uv`, `rust-cargo`, `js-ts`, or `generic`). Review the presets and commit the manifest with the project so both devices share the same configuration. Generated JavaScript preflights check the package manager; when a test or build needs installed dependencies, its project script must prepare them inside the task worktree.

To try the forecasting fixture from the benchmark archive, make a standalone copy of `course/bench/fixture-repo` from `HEAD` in a fresh `.tmp/relay-fixture` directory.

```powershell
New-Item -ItemType Directory -Force .tmp | Out-Null
git archive --format=zip --output=.tmp/relay-fixture.zip HEAD:course/bench/fixture-repo
Expand-Archive -LiteralPath .tmp/relay-fixture.zip -DestinationPath .tmp/relay-fixture
git -C .tmp/relay-fixture init
git -C .tmp/relay-fixture add --all
git -C .tmp/relay-fixture commit -m "chore(bench): initialize forecast fixture"

$env:RELAY_REPO_ROOT = (Resolve-Path .tmp/relay-fixture).Path
cd crates/relay-desktop
bun run tauri dev
```

Use the `smoke` preset for a quick run or `train_grid` for a window sweep. The [fixture manifest](../course/bench/fixture-repo/relay.toml) defines the accepted parameters and timeouts. Set or clear `RELAY_REPO_ROOT` before launching a different checkout.

For model-backed diagnosis, start Ollama and pull the default model.

```powershell
ollama pull qwen3.5:4b
```

## Run the headless daemon

Run these examples from the repository root. Each command runs in the foreground. Use Ctrl+C to stop the network servers.

```powershell
# Verify a loopback ping between two nodes with separate data directories.
cargo run -p relay-daemon -- sandbox-dual

# Serve the Host with P2P and Streamable HTTP MCP at http://127.0.0.1:7608/mcp.
cargo run -p relay-daemon -- --repo-root .tmp/relay-fixture serve

# Serve MCP alone over HTTP.
cargo run -p relay-daemon -- --repo-root .tmp/relay-fixture mcp-http --addr 127.0.0.1:7608

# Use this command and these arguments in a stdio MCP client.
cargo run -p relay-daemon -- --repo-root .tmp/relay-fixture mcp-stdio
```

Use `serve --lan-only` to pair devices over direct IP when the LAN is available and public relay services are offline. Pass the printed pairing ticket to the other device. For two processes on the same machine, `serve --local-only` binds the P2P endpoint to loopback. Both flags affect P2P transport. The HTTP MCP listener defaults to `127.0.0.1:7608` and can be configured independently with `--mcp-addr`. The daemon stores its data under `.relay/` by default. Pass `--data-dir` for separate state and `--repo-root` for the checkout used by MCP worktree tools. Set the MCP client's working directory to the repository root when using the relative paths above.

## Verification

Run the checks relevant to your change before committing. Expect zero warnings and errors.

| Change | Working directory | Check |
| --- | --- | --- |
| Rust behavior | Repository root | `cargo test -p relay-core -p relay-daemon` and `cargo check --workspace` |
| Shared contracts or Tauri commands and events | `crates/relay-desktop` | `bun run gen:bindings`, review the generated diff, and run the Rust and frontend checks |
| Desktop frontend | `crates/relay-desktop` | `bun run format:check`, `bun run lint`, `bun test`, and `bun run build` |
| Evaluation & benchmark scripts | Repository root | `uv run python -m unittest discover -s course/eval -p "test_*.py"` and `uv run python -m unittest discover -s course/bench/results -p "test_*.py"` |
| Documentation only | Repository root | Verify links and examples against their source files, then run `git diff --check` |

- `bun run build` runs TypeScript checking and Vite. Vite clears `dist/`, so restore the tracked placeholder from the repository root with `git restore --worktree -- crates/relay-desktop/dist/.gitkeep` after the build. A fresh checkout needs that path for `cargo check --workspace`.
- The binding export script runs the `export_bindings` binary. On Windows that binary includes the ComCtl32 v6 manifest required by Tauri. Keep Rust unit and integration checks in the `relay-core` and `relay-daemon` test suites.
- For changes to worktree execution, approvals, or peer sync, exercise the affected path with a standalone fixture and local endpoints. For UI changes, check the affected flow in English and Simplified Chinese.
- Check [pinned Rust API patterns](api-cheatsheet.md) against the dependency versions in the workspace. SQLx queries take static SQL, and Tauri-Specta types use `u32` and the shared `JsonValue` for portable numeric and JSON fields. In MCP handlers, use `ServerConfig`, `ContentBlock::text`, the tool router, and `rmcp::elicit_safe!` for elicitation.

## Branches and commits

Work on a short-lived branch such as `fix/desktop-pairing` and open pull requests into `main`. Keep each commit focused on one logical change. Use an imperative lowercase summary, at most 72 characters for the header and no trailing period.

```text
<type>(<scope>): <summary>

docs(desktop): document loopback development setup
```

- Types include `feat`, `fix`, `refactor`, `docs`, `test`, `bench`, `perf`, `chore`, and `ci`.
- Standard scopes are `core`, `sandbox`, `sync`, `mcp`, `agent`, `daemon`, `desktop`, `i18n`, `contracts`, `bench`, `chaos`, `prompts`, `eval`, `oss`, and `docs`.
- Keep runtime state (`.relay/`, `.relay-desktop/`, `*.db`, `*.db-wal`, `*.db-shm`), credentials (`.env`, `.env.*`, keys and tokens), and generated build or scratch files (`target/`, `node_modules/`, `dist/`, `.venv/`, `.tmp/`) out of commits. Keep `crates/relay-desktop/dist/.gitkeep` tracked.

See the [Conventional Commits specification](https://www.conventionalcommits.org/en/v1.0.0/) for commit bodies and footers.
