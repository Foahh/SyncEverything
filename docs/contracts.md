# SyncEverything (Relay Engine) Interface Contracts

> **Stability:** Core interface contracts shared across the Rust contract structs (`crates/relay-core/src/contracts/`), MCP JSON schemas, and auto-generated Tauri-Specta TypeScript bindings (`crates/relay-desktop/src/bindings.ts`).

---

## 1. MCP Tool Contract (`rmcp` 3.11)

Exposed over both **Stdio** (`relay-daemon mcp-stdio`, compatible with Claude Code, Cursor, Codex, `dsh`, and any standard MCP client) and **Streamable HTTP** (`relay-daemon mcp-http --addr 127.0.0.1:7608`, mounted at `/mcp`).

### Read-Only Tools
| Tool Name | Arguments | Purpose |
|---|---|---|
| `inspect_sync_status` | `{}` | Returns current runtime mode, local/peer `NodeId`, transport path (`DirectLan` / `Relay` / `Offline`), last synced commit, and pending outbox count. |
| `list_handoffs` | `{ limit?: u32 }` | Lists recent cross-device handoff records with base commit SHA, summary note, and active task states. |
| `check_env_diff` | `{ commit_sha: String }` | Compares target commit's dependency manifest and lockfile fingerprint against the Host environment across supported package managers (`uv`, `cargo`, `bun`, `pnpm`, `yarn`, `npm`, `generic`), reporting `ecosystem`, `package_manager`, `missing_packages`, and change notes. |
| `get_dag_snapshot` | `{}` | Returns the complete Decision-Evidence DAG (`nodes`, `edges`, `stale_count`, `fresh_evidence_count`, `stale_evidence_count`, `metric_authority_note`) with explicit `STALE` freshness flags and deterministically quarantines/redacts `metrics_json` on `STALE` evidence nodes at the MCP boundary so agents treat the DAG as the sole metric SSOT and never cite outdated metrics. |
| `read_evidence_log` | `{ node_id: String, max_lines?: u32 }` | Reads artifact/evidence metrics and truncated log blob; deterministically blocks `STALE` evidence nodes. |
| `read_worktree_file` | `{ task_id: String, file_path: String, start_line?: u32, end_line?: u32 }` | Reads a UTF-8 file with 1-based line numbers strictly inside the task's isolated Git worktree (blocks `..`, symlink escape, `.git/`, `.venv/`, `.relay/`, `.env*`, and binary files). |

### Isolated Sandbox & Self-Healing Tools
| Tool Name | Arguments | Purpose |
|---|---|---|
| `run_preflight_in_worktree` | `{ task_id: String }` | Checks out `base_commit_sha` into an isolated `git worktree`, reconciles `uv.lock`, and runs the repo's `smoke` preset from `relay.toml`. On failure, captures `stderr_tail`, `detected_missing_module`, and automatic ±10-line `error_snippets` around worktree-local error lines without touching the main working tree. |
| `propose_worktree_patch` | `{ task_id: String, patch_kind: "dependency_spec" \| "code_diff", summary: String, dependency_spec?: String, edits?: Array<{ file_path, old_text?, new_text }> }` | Applies a structured package-manager dependency addition (`dependency_spec`) or atomic exact search-and-replace / full-file overwrite edits (`code_diff`) inside the isolated worktree, re-runs smoke preflight (automatically rolling back tracked and untracked changes if smoke fails), generates a standard `git diff` `diff_preview`, persists a `PatchProposal` in SQLite, and transitions the task to `SelfHealingProposed`. |

### Human-in-the-Loop Execution Tool (Dual-Mode Elicitation)
| Tool Name | Arguments | Purpose |
|---|---|---|
| `approve_and_execute_task` | `{ task_id: String, patch_id?: String, approved_via_ui?: bool }` | Supports **both** native MCP Elicitation (when connected client supports it) and persistent offline-resilient approval: if not yet approved, returns `ApprovalRequired { task_id, patch_id, diff_preview }` so `relay-desktop` can show the Diff approval modal on reconnect and commit approval via Tauri command or subsequent MCP call. |

---

## 2. Decision-Evidence DAG Contract (`contracts::dag`)

* **`NodeKind`**:
  * `Decision { rationale: String, constraints: Vec<String> }`
  * `Artifact { git_commit_sha: String, env_fingerprint: String, working_tree_dirty: bool, changed_files: Vec<String> }`
  * `Evidence { task_id: String, preset: String, metrics_json: serde_json::Value, log_blob_hash: String }`
* **`EdgeKind`**: `Informs` (`Decision -> Artifact`), `ProducedFrom` (`Artifact -> Evidence`), `Supersedes` (`Artifact -> Artifact`), `DerivedFrom` (`Evidence -> Decision`).
* **`Freshness`**:
  * `Fresh`
  * `Stale { caused_by: NodeId, reason: String, marked_at: DateTime<Utc> }`
  * `PendingVerify`
* **Deterministic Anti-Hallucination Rule:** Whenever any `Artifact` node is updated or superseded by a newer `Artifact`, `DagEngine` performs a deterministic BFS/DFS traversal along downstream edges and marks all reachable `Evidence` nodes as `Freshness::Stale`. Agent context builders MUST exclude or explicitly quarantine `STALE` evidence nodes.

---

## 3. Task Spec, State Machine & `relay.toml` Whitelist (`contracts::task`)

* **`TaskSpec`**:
  * `task_id: Uuid` (UUIDv7, globally unique & time-ordered for idempotency)
  * `repo_id: String`
  * `base_commit_sha: String`
  * `command_preset: String` (must exist in target repo's `relay.toml` `[presets.<name>]`)
  * `params: BTreeMap<String, String>`
  * `require_approval: bool`
* **`TaskState`** transitions:
  `Queued` → `PreflightRunning` → `SelfHealingProposed` *(paused awaiting Elicitation / UI Diff Approval)* → `Executing` → `Completed` | `Failed` | `Stale` | `Rejected` (plus `Queued` | `PreflightRunning` | `Executing` → `Cancelled` on explicit local or remote cancellation, persisting partial logs)
* **Security Boundary (`relay.toml`)**:
  The Host daemon NEVER executes arbitrary shell strings over P2P or MCP. Every runnable task must resolve to a named preset in `<repo>/relay.toml` with explicit `program`, `args`, `timeout_secs`, and optional `allowed_params`. `[project]` may optionally pin `package_manager` (`uv`, `cargo`, `bun`, `pnpm`, `yarn`, `npm`, or `generic`), validated strictly on manifest load.

---

## 4. Wire Protocol Contract (`iroh` 1.5 Single Endpoint, `contracts::wire`)

A single `iroh::Endpoint` handles both direct LAN discovery and NAT-traversing relay routing, registering two ALPNs on `iroh::protocol::Router`:
1. **`iroh_blobs::ALPN`** (wrapped in `RevocationGuardedBlobs`, rejecting revoked peers before accepting blob streams): Content-addressed BLAKE3 transfer of:
   * Git bundles (`git bundle create`) carrying exact commit history & SHAs
   * Uncommitted working tree delta patches (`git diff` + untracked file archive)
   * Experiment stdout/stderr logs & metric artifacts
2. **`b"relay/rpc/1"`** (`RELAY_RPC_ALPN`, gated by the Host revocation check returning `WireResponse::Error { code: "PEER_REVOKED", ... }` before dispatching any request from a revoked `NodeId` in `trusted_peers`): Length-prefixed JSON frames over QUIC bidirectional streams for:
   * `Ping` / `PeerStatus`
   * `PushSyncBundle` (announces blob hash + commit metadata)
   * `SyncDagDelta` (incremental node/edge/freshness updates)
   * `SubmitTask` / `CancelTask` / `ApprovePatch` / `RejectPatch`
   * `FetchTaskState`

---

## 5. Runtime Modes (`RuntimeMode`)

`relay-desktop` supports three runtime modes so single-machine development and dual-machine (Windows Host + macOS Client) workflows use the exact same code paths:
1. `Client` — Laptop workbench connecting to a remote Host `NodeId`
2. `Host` — Workstation UI inspecting and controlling the local Relay engine
3. `LocalDualSandbox` — Spawns isolated local Client + Host endpoints on one machine over loopback `iroh` for fast end-to-end testing

---

## 6. Chaos Suite Contract (`contracts::chaos`)

Every fault-injection case in `course/bench/chaos-cases/*.json` conforms to `ChaosCase`:
* `case_id`, `title`, `description`
* `initial_commit`, `injected_fault` (`MissingDependency` | `UpstreamCodeEdit` | `SameLineConflict` | `MidRunDisconnect` | `RuntimeException` | `CorruptedEnvLock` | `StaleEvidenceTrap` | `DuplicateTaskReplay`)
* `user_prompt`
* `expected_tool_trace: Vec<String>`
* `forbidden_hallucinations: Vec<String>` (e.g., citing `STALE` metrics)
* `ground_truth_assertions: Vec<ChaosAssertion>`

---

## 7. Frontend Localization & Type Binding Contract

* **Bilingual i18n**: `react-i18next` + `i18next` (`en` and `zh-CN`), switching live in the top bar without losing DAG canvas state.
* **Strict Type Safety**: Frontend code in `crates/relay-desktop/src/` MUST ONLY call Tauri commands and subscribe to events via auto-generated `crates/relay-desktop/src/bindings.ts` (exported by `tauri-specta`). Hand-written `invoke("...")` calls are forbidden.
