# Relay v1.0 Frozen Day 1 Interface Contracts

> **Status:** Frozen on Day 1. All 6 parallel tracks build against these exact Rust structs (`crates/relay-core/src/contracts/`), JSON schemas, and Tauri-Specta TypeScript bindings (`crates/relay-desktop/src/bindings.ts`).

---

## 1. MCP Tool Contract (`rmcp` 3.11)

Exposed over both **Stdio** (`relay-daemon mcp-stdio`, compatible with Claude Code, Cursor, Codex, `dsh`, and any standard MCP client) and **Streamable HTTP** (`relay-daemon mcp-http --addr 127.0.0.1:7608`, mounted at `/mcp`).

### Read-Only Tools
| Tool Name | Arguments | Purpose |
|---|---|---|
| `inspect_sync_status` | `{}` | Returns current runtime mode, local/peer `NodeId`, transport path (`DirectLan` / `Relay` / `Offline`), last synced commit, and pending outbox count. |
| `list_handoffs` | `{ limit?: u32 }` | Lists recent cross-device handoff records with base commit SHA, summary note, and active task states. |
| `check_env_diff` | `{ commit_sha: String }` | Compares target commit's `pyproject.toml` / `uv.lock` fingerprint against the Host environment fingerprint. |
| `get_dag_snapshot` | `{ include_stale?: bool }` | Returns the Decision-Evidence DAG (`nodes`, `edges`, `stale_count`) with explicit `STALE` freshness flags and deterministically quarantines/redacts `metrics_json` on `STALE` evidence nodes at the MCP boundary so agents never cite outdated metrics. |
| `read_evidence_log` | `{ node_id: String, max_lines?: u32 }` | Reads artifact/evidence metrics and truncated log blob; returns a prominent warning header if the evidence node is `STALE`. |

### Isolated Sandbox & Self-Healing Tools
| Tool Name | Arguments | Purpose |
|---|---|---|
| `run_preflight_in_worktree` | `{ task_id: String }` | Checks out `base_commit_sha` into an isolated `git worktree`, reconciles `uv.lock`, and runs the repo's `smoke` preset from `relay.toml`. Captures traceback/missing dependencies without touching the main working tree. |
| `propose_worktree_patch` | `{ task_id: String, patch_kind: "uv_spec" \| "code_diff", summary: String, content: String }` | Applies a candidate fix inside the isolated worktree, re-runs smoke preflight, persists a `PatchProposal` in SQLite, and transitions the task to `SelfHealingProposed`. |

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
  `Queued` → `PreflightRunning` → `SelfHealingProposed` *(paused awaiting Elicitation / UI Diff Approval)* → `Executing` → `Completed` | `Failed` | `Stale` | `Rejected`
* **Security Boundary (`relay.toml`)**:
  The Host daemon NEVER executes arbitrary shell strings over P2P or MCP. Every runnable task must resolve to a named preset in `<repo>/relay.toml` with explicit `program`, `args`, `timeout_secs`, and optional `allowed_params`.

---

## 4. Wire Protocol Contract (`iroh` 1.5 Single Endpoint, `contracts::wire`)

A single `iroh::Endpoint` handles both LAN direct discovery and campus-Wi-Fi relay NAT traversal, registering two ALPNs on `iroh::protocol::Router`:
1. **`iroh_blobs::ALPN`**: Content-addressed BLAKE3 transfer of:
   * Git bundles (`git bundle create`) carrying exact commit history & SHAs
   * Uncommitted working tree delta patches (`git diff` + untracked file archive)
   * Experiment stdout/stderr logs & metric artifacts
2. **`b"relay/rpc/1"`** (`RELAY_RPC_ALPN`): Length-prefixed JSON frames over QUIC bidirectional streams for:
   * `Ping` / `PeerStatus`
   * `PushSyncBundle` (announces blob hash + commit metadata)
   * `SyncDagDelta` (incremental node/edge/freshness updates)
   * `SubmitTask` / `ApprovePatch` / `RejectPatch`
   * `FetchTaskState`

---

## 5. Runtime Modes (`RuntimeMode`)

`relay-desktop` supports three runtime modes so single-machine development and dual-machine (Windows Host + macOS Client) live demos use the exact same code paths:
1. `Client` — Laptop workbench connecting to a remote Host `NodeId`
2. `Host` — Workstation UI inspecting and controlling the local `relay-daemon`
3. `LocalDualSandbox` — Spawns isolated local Client + Host endpoints on one machine over loopback `iroh` for fast end-to-end testing

---

## 6. Chaos Suite Contract (`contracts::chaos`)

Every Track 4 fault-injection case in `bench/chaos-cases/*.json` conforms to `ChaosCase`:
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
