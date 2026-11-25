# SyncEverything (Relay Engine)

## Project references

- Use [architecture](docs/architecture.md) for component boundaries, [development guide](docs/development.md) for build and verification workflows, and [product roadmap](docs/product-roadmap.md) for release scope.
- Use [contracts](docs/contracts.md) for shared interfaces, the [Rust API cheatsheet](docs/api-cheatsheet.md) for pinned dependencies, and the [desktop design spec](docs/frontend-design-spec.md) for UI behavior and terminology.
- Historical academic artifacts, benchmarks, and evaluation scripts live under [course/](course/README.md).

## Engineering constraints

- Do not keep backward-compatibility shims, legacy fallbacks, or deprecated aliases—this is a greenfield project. When upstream compatibility, contracts, or data formats change, migrate callers and state manually.
- Preserve the shared interfaces in `crates/relay-core/src/contracts/` unless the task explicitly changes them.
- Host task execution must use a named preset and validate parameters against `allowed_params` in the target repository's `relay.toml`. Never execute arbitrary shell strings.
- Invoke system `git` and `uv` through `tokio::process::Command`. Use `reqwest` for inference via OpenAI-compatible `/chat/completions`. Do not add `git2` or local Rust inference crates.
- Keep `mcp-stdio` stdout exclusively for JSON-RPC. Send logs to stderr. Use `NetworkProfile::LocalOnly` for offline peer tests.
- Use Bun for desktop tooling and route Tauri IPC through `src/lib/relayApi.ts` and generated bindings. Run `bun run gen:bindings` from `crates/relay-desktop` when Tauri commands, events, or exported contracts change. Never edit `src/bindings.ts` by hand.
- Add user-facing strings to both `en.json` and `zh-CN.json`. Follow the desktop design spec for product copy and placement of developer controls.

## Verification and commits

- Run the checks for the affected area in the [development guide](docs/development.md#verification). After a desktop build, restore the tracked `crates/relay-desktop/dist/.gitkeep`.
- Follow the [branch and commit conventions](docs/development.md#branches-and-commits). Keep secrets, machine-specific paths, local state, and generated build output out of commits.
