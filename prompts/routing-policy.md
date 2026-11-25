# SyncEverything — Asymmetric Dual-Agent Routing Policy

SyncEverything (powered by the Relay engine) separates deterministic state management (Rust DAG, SQLite WAL, `git worktree` + extensible package manager adapter sandbox) from LLM reasoning, and routes inference across three tiers depending on task complexity, latency budget, privacy, and offline availability.

## Tier Selection Matrix

| Tier | Default Target | When Selected | Allowed Tools / Scope |
|---|---|---|---|
| **Edge Fast-Path** | Local small model (`qwen3.5:4b`) or deterministic parser on laptop | Sync status checks, handoff summaries, `STALE` checks, note capture while offline or on battery | Read-only MCP tools (`inspect_sync_status`, `list_handoffs`, `get_dag_snapshot`, `read_evidence_log`, `check_env_diff`) |
| **Host Local Ollama** | `qwen3.5:4b` / `qwen3.5:9b` via `http://127.0.0.1:11434/v1` | Default workstation preflight diagnosis, missing-dependency repair, offline / strict-LAN privacy mode | All nine MCP tools, including `run_preflight_in_worktree`, `read_worktree_file`, `propose_worktree_patch`, and `approve_and_execute_task` |
| **Cloud OpenAI-Compatible API** | User-configured endpoint (e.g., DeepSeek, OpenAI, OpenRouter) | Multi-file traceback diagnosis and `code_diff` repair with structured file edits, long log analysis, or model comparison | All nine MCP tools, with stale and unverified metrics redacted |

## Deterministic Fallbacks & Guardrails

1. **Missing Dependency Offline Fallback**: If the configured Host LLM is unreachable (connection refused / timeout) and `PreflightReport.detected_missing_module` is populated, `SelfHealingAgent` deterministically maps known Python imports (e.g., `sklearn -> scikit-learn`, `yaml -> pyyaml`, `cv2 -> opencv-python`) to a `dependency_spec` proposal and verifies it in the isolated worktree before surfacing it for user approval.
2. **Stale Quarantine at System Layer**: Even if a smaller quantized model attempts to call `read_evidence_log` on an outdated node, `RelayEngine` and `RelayMcpServer` reject the read deterministically with `StaleEvidenceBlocked`, preventing stale-metric hallucinations independently of model capability.
3. **Approval Gate at System Layer**: Tasks marked `require_approval = true` (or carrying any self-healing patch) cannot execute without either live MCP Elicitation confirmation or explicit UI Diff Approval (`resolve_patch_approval`).
