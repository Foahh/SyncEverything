# Verified Late-2026 Rust API Cheatsheet (for Codex CLI & Contributors)

> **Purpose:** Prevent AI coding agents from hallucinating outdated `iroh` 0.x, `rmcp` 0.x, or `tauri-specta` 1.x APIs. Every version and snippet below is verified against our workspace `Cargo.toml` (`rustc 1.100.0`, `edition = "2024"`).

---

## 1. Exact Pinned Workspace Crates

| Crate | Version | Notes |
|---|---|---|
| `iroh` | `1.5.0` | Stable 1.x QUIC P2P endpoint + router |
| `iroh-blobs` | `0.105.0` | Compatible with `iroh ^1.0.0`, `features = ["fs-store"]` |
| `rmcp` | `3.11.0` | Official Rust MCP SDK (`server`, `macros`, `transport-io`, `transport-streamable-http-server`, `elicitation`, `schemars`) |
| `axum` | `0.8.9` | Mounts `StreamableHttpService` at `/mcp` |
| `sqlx` | `0.9.1` | `runtime-tokio`, `sqlite`, `sqlite-bundled`, `chrono`, `uuid`, `json`, `migrate` |
| `petgraph` | `0.8.3` | In-memory DAG & deterministic STALE traversal |
| `notify` | `9.0.0` | Cross-platform filesystem watcher |
| `tauri` / `tauri-build` | `2.12.7` / `2.7.6` | Tauri v2 stable |
| `tauri-specta` | `=2.0.0-rc.29` | Requires exact `specta = "=2.0.0-rc.28"` & `specta-typescript = "0.0.15"` |
| `reqwest` | `0.13.5` | Use feature `"rustls"` (NOT `"rustls-tls"`) |
| `schemars` | `1.2.3` | Default features only (`JsonSchema` derive); use `String` for UUID/DateTime tool inputs |

---

## 2. Hard Engineering Rules (Non-Negotiable)

1. **NO `git2` (libgit2) and NO local Rust LLM inference crates (`candle`, `llama-cpp-rs`).**
   * Git operations (`worktree`, `bundle`, `diff`, `status`, `rev-parse`) MUST call system `git` asynchronously via `tokio::process::Command`.
   * Python environment reconciliation & execution MUST call `uv` asynchronously via `tokio::process::Command`.
   * LLM inference MUST call OpenAI-compatible HTTP endpoints via `reqwest` (`http://127.0.0.1:11434/v1` for Ollama `qwen3.5:4b/9b`, or configurable Cloud API base URL).
2. **NO arbitrary shell execution on Host.** Always parse `<repo>/relay.toml` and execute whitelisted presets only.
3. **NO hand-written Tauri `invoke("...")` strings in TypeScript.** Always regenerate `crates/relay-desktop/src/bindings.ts` via `tauri-specta` whenever commands or event payloads change.

---

## 3. `iroh` 1.5 + `iroh-blobs` 0.105 Pattern

```rust
use iroh::{
    Endpoint,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobsProtocol, store::fs::FsStore};

// Bind endpoint with default N0 relay + discovery preset
let endpoint = Endpoint::bind(presets::N0).await?;
endpoint.online().await;

// Persistent BLAKE3 blob store
let store = FsStore::load(blob_dir).await?;
let blobs = BlobsProtocol::new(&store, None);

// Mount BOTH iroh-blobs and custom relay/rpc/1 on ONE Endpoint
let router = Router::builder(endpoint)
    .accept(iroh_blobs::ALPN, blobs)
    .accept(RELAY_RPC_ALPN, my_rpc_handler)
    .spawn();
```

---

## 4. `rmcp` 3.11 Server Pattern

```rust
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};

#[derive(Clone)]
pub struct RelayMcpServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl RelayMcpServer {
    #[tool(description = "...")]
    async fn inspect_sync_status(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text("{}")]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RelayMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
```

---

## 5. `tauri-specta` `2.0.0-rc.29` Pattern

```rust
use specta_typescript::Typescript;
use tauri_specta::{Builder, Event, collect_commands, collect_events};

#[tauri::command]
#[specta::specta]
async fn get_dag_snapshot(...) -> Result<DagSnapshot, String> { ... }

#[derive(Clone, Debug, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct RelayStatusEvent { ... }

let builder = Builder::<tauri::Wry>::new()
    .commands(collect_commands![get_dag_snapshot])
    .events(collect_events![RelayStatusEvent]);

builder.export(Typescript::default(), "../src/bindings.ts")?;
```

> **Windows, `sqlx 0.9` & Specta v2 Notes:**
> 1. `tauri_build::build()` embeds the Windows ComCtl32 v6 manifest into `[[bin]]` targets, not `lib` unit-test harnesses. Export bindings via `cargo run -p relay-desktop --bin export_bindings -- crates/relay-desktop/src/bindings.ts` and keep unit tests in `relay-core` / `relay-daemon`.
> 2. `specta-typescript 0.0.15` forbids `BigInt`-sized integers (`i64`/`u64`/`usize`, including `serde_json::Value`). Use `relay_core::contracts::JsonValue` (backed by `f64`) and `u32` in all `#[derive(specta::Type)]` structs!
> 3. `sqlx 0.9.1` enforces `SqlSafeStr`: always pass `&'static str` literals (or `const &str`) to `sqlx::query` / `sqlx::query_as`, never `format!(...)` strings.
