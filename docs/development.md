# Relay development guide

Use this guide to launch the workbench, run the daemon locally, and verify a change. See the [architecture guide](architecture.md) to find the component that owns a behavior.

## Prerequisites

| Work | Required tools |
| --- | --- |
| Rust workspace | Git and a Rust toolchain compatible with the workspace. [`Cargo.toml`](../Cargo.toml) declares Rust 1.94 as the workspace minimum and uses edition 2024. Use [`Cargo.lock`](../Cargo.lock) for resolved dependency versions. |
| Desktop frontend | [Bun](https://bun.sh) (`1.3+`). Use [`package.json`](../crates/relay-desktop/package.json) for available scripts and [`bun.lock`](../crates/relay-desktop/bun.lock) for resolved dependencies. |
| Native desktop and binding export | Windows requires the MSVC Rust toolchain, Microsoft C++ Build Tools, and WebView2. On macOS, install the Xcode Command Line Tools for desktop development. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). |
| Forecast fixture and worktree preflight | System Git and `uv` on `PATH`, plus Python 3.11 or newer as specified by the [fixture](../bench/fixture-repo/pyproject.toml). |
| Local model inference | Ollama running on `127.0.0.1:11434`. The desktop diagnosis path uses `qwen3.5:4b` by default. The [model evaluation track](../eval/models/) also uses `qwen3.5:9b`. |

Use an ignored directory such as `.tmp/` for scratch files and `.relay/` for runtime state.

## Start the desktop

From the repository root, install the locked frontend dependencies and launch Tauri.

```powershell
cd crates/relay-desktop
bun install --frozen-lockfile
bun run tauri dev
```

On a fresh desktop data directory, Relay starts in `LocalDualSandbox`. The selected mode persists across restarts. Set `RELAY_DATA_DIR` before launch to use separate desktop state for a test or demo.

For browser-only UI work, run `bun run dev` from `crates/relay-desktop` and open `http://localhost:1420`. The browser uses in-memory preview data. Use Tauri for checks that exercise native IPC, storage, or networking.

## Prepare a fixture for task execution

The checked-in fixture lives inside the Relay monorepo. The desktop requires a separate committed Git checkout with `relay.toml` at its root before it enables worktree execution. From the Relay root, use a fresh `.tmp/relay-fixture` to make a standalone copy of the fixture from `HEAD`.

```powershell
New-Item -ItemType Directory -Force .tmp | Out-Null
git archive --format=zip --output=.tmp/relay-fixture.zip HEAD:bench/fixture-repo
Expand-Archive -LiteralPath .tmp/relay-fixture.zip -DestinationPath .tmp/relay-fixture
git -C .tmp/relay-fixture init
git -C .tmp/relay-fixture add --all
git -C .tmp/relay-fixture commit -m "chore(bench): initialize forecast fixture"

$env:RELAY_REPO_ROOT = (Resolve-Path .tmp/relay-fixture).Path
cd crates/relay-desktop
bun run tauri dev
```

Use the `smoke` preset for a quick run or `train_grid` for a window sweep. The [fixture manifest](../bench/fixture-repo/relay.toml) defines the accepted parameters and timeouts. Set or clear `RELAY_REPO_ROOT` before launching a different checkout.

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

Use `serve --local-only` to bind the P2P endpoint to loopback with relays disabled. The daemon stores its data under `.relay/` by default. Pass `--data-dir` for separate state and `--repo-root` for the checkout used by MCP worktree tools. Set the MCP client’s working directory to the Relay repository root when using the relative paths above.

## Verification

Run the checks relevant to your change before committing. Expect zero warnings and errors.

| Change | Working directory | Check |
| --- | --- | --- |
| Rust behavior | Repository root | `cargo test -p relay-core -p relay-daemon` and `cargo check --workspace` |
| Shared contracts or Tauri commands and events | `crates/relay-desktop` | `bun run gen:bindings`, review the generated diff, and run the Rust and frontend checks |
| Desktop frontend | `crates/relay-desktop` | `bun run format:check`, `bun run lint`, and `bun run build` |
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
- Standard scopes are `core`, `sandbox`, `sync`, `mcp`, `agent`, `daemon`, `desktop`, `i18n`, `contracts`, `bench`, `chaos`, `prompts`, `eval`, and `docs`.
- Keep runtime state (`.relay/`, `.relay-desktop/`, `*.db`, `*.db-wal`, `*.db-shm`), credentials (`.env`, `.env.*`, keys and tokens), and generated build or scratch files (`target/`, `node_modules/`, `dist/`, `.venv/`, `.tmp/`) out of commits. Keep `crates/relay-desktop/dist/.gitkeep` tracked.

See the [Conventional Commits specification](https://www.conventionalcommits.org/en/v1.0.0/) for commit bodies and footers.
