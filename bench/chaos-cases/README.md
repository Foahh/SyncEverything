# Chaos Suite — Track 4 Fault-Injection Benchmark

Eight declarative fault-injection cases plus a dual-mode judge (`scripted` offline CI gate and live `llm` coding-agent eval) and a P2P loopback connectivity gate (`relay-daemon sandbox-dual`). Every JSON file conforms strictly to the frozen `relay_core::contracts::ChaosCase` schema (`docs/contracts.md` §6).

## Coverage (8/8 Fault Kinds · 8/8 MCP Tools)

| Case | `injected_fault.kind` | Scenario | Expected tool trace | Core assertions |
|---|---|---|---|---|
| `chaos-01-missing-dep` | `missing_dependency` | Undeclared `tiny_stats` import → offline `uv_spec` wheel repair | check env → preflight → propose → approve | missing module detected, real `ApprovalRequired` gate verified, 1 fresh evidence, main tree clean |
| `chaos-02-upstream-edit` | `upstream_code_edit` | Laptop commits a new alpha default after the host result | preflight → approve → snapshot → read log | 1 stale evidence, 0 fresh, task stays `completed`, `STALE` error block enforced |
| `chaos-03-same-line-conflict` | `same_line_conflict` | Laptop holds an uncommitted edit on the patched line | preflight → propose → approve | real `git apply` fails on client, client edit byte-preserved, host tree clean |
| `chaos-04-mid-run-disconnect` | `mid_run_disconnect` | Daemon killed cleanly mid-smoke preflight, then restarted | preflight → sync status → handoffs → approve | state persisted `preflight_running`, preflight resumes, 1 fresh evidence, task `completed` |
| `chaos-05-runtime-exception` | `runtime_exception` | Undefined `DEFAULT_ALPHA` NameError in smoke | preflight → propose → approve | `NameError` detected in preflight, smoke verified after `code_diff`, `best_window = 28`, task `completed` |
| `chaos-06-corrupted-lock` | `corrupted_env_lock` | Truncated `uv.lock` fails TOML parsing | preflight → propose → approve | parse failure detected, lock repair verified, task `completed` |
| `chaos-07-stale-evidence-trap` | `stale_evidence_trap` | Results requested after an upstream invalidation | snapshot → read log | `STALE` read refused, DAG bundle exposes 0 stale metrics, full transcript exposes 1 |
| `chaos-08-duplicate-replay` | `duplicate_task_replay` | Client replays an approval after a lost response | preflight → approve → snapshot | replay answers `execution_started`, 1 evidence, task stays `completed` |

## One-Click Dual-Mode Judge

```bash
# Default: deterministic offline regression + P2P loopback gate
./bench/chaos-cases/judge.sh
uv run python bench/chaos-cases/run_chaos_suite.py

# Live LLM evaluation mode (auto-loads RELAY_LLM_* from repo-root .env)
./bench/chaos-cases/judge.sh --mode llm
uv run python bench/chaos-cases/run_chaos_suite.py --mode llm
```

Useful flags:

```bash
# Single case, keeping scratch repo and --data-dir for inspection
uv run python bench/chaos-cases/run_chaos_suite.py --case chaos-04-mid-run-disconnect --keep

# Explicit binary, report directory, or skipping the P2P loopback gate
uv run python bench/chaos-cases/run_chaos_suite.py --daemon-bin target/debug/relay-daemon.exe --out bench/chaos-cases/results
```

- `--mode scripted` (default): writes `results/chaos-report.json` and `results/chaos-report.md`, enforces `tool_recall == 1.0`, verifies 8/8 frozen MCP tool coverage, and runs `relay-daemon sandbox-dual` as an `iroh` loopback connectivity gate (unless `--skip-p2p-gate`).
- `--mode llm`: equips an OpenAI-compatible LLM (`RELAY_LLM_BASE_URL` / `RELAY_LLM_MODEL` / `RELAY_LLM_API_KEY` from `.env` or environment) with all 8 live MCP tools plus repo-confined read-only helpers (`list_repo_files`, `read_repo_file`), scoring self-healing success rate, tool recall, retry overhead, STALE anti-hallucination immunity, tokens, and latency in `results/chaos-llm-report.{json,md}` for Track 5.

Latest verified runs (Windows workstation):
- **Scripted + P2P Gate**: **8/8 PASS** (`tool_recall = 1.00`, `8/8` MCP tools covered, `sandbox-dual` loopback **PASS** in `2117.4 ms`; see `results/chaos-report.md`).
- **Live Cloud LLM (`deepseek-flash`)**: **8/8 PASS**, Self-Heal **4/4 (100%)**, Avg Tool Recall **1.000**, Retries **0**, Stale Hallucinations **0** (see `results/chaos-llm-report.md`).

## How a Case Is Executed

1. The fixture is copied into a scratch directory with LF-normalized text files (`core.autocrlf = false`), `git init`-ed, and committed cleanly (`initial_commit: "HEAD"`).
2. The fault recipe injects the failure and submits a `TaskSpec` via `relay-daemon submit-task`.
3. `relay-daemon mcp-stdio` is driven over JSON-RPC (`--mode scripted` deterministically; `--mode llm` via the live OpenAI-compatible tool loop with simulated human UI diff approval once `approval_required` is observed).
4. Metrics come from four real sources: MCP tool outputs, read-only SQLite inspection (`--data-dir/relay.db`), `git status` / `git apply` on checkouts, and `relay-daemon compose-context --all-strategies --no-sync`.
5. Every `ground_truth_assertions` entry is checked with strict equality (bools strict, numbers numeric). Case 3 additionally executes a real client `git apply` (not `--check`) against a dirty laptop checkout (`default=0.55`) and verifies exact byte preservation.

## Determinism & Offline Policy

- `UV_OFFLINE=1`, `UV_NO_CONFIG=1`, and cleared index variables ensure zero PyPI traffic and keep machine-local mirror URLs out of proposal diffs.
- Case 1 repairs `tiny_stats` offline using `vendor/tiny_stats-0.1.0-py3-none-any.whl`.
- Every case runs in an isolated temporary repo + `--data-dir`, cleaned up automatically unless `--keep` is passed.
- `judge.sh` supports Windows Git Bash / MSYS (`.exe` detection + `uv run python`), macOS, and Linux.

## Adding a Case

1. Add `case_NN_<slug>.json` with a `kind` from `InjectedFaultKind` (`docs/contracts.md` §6) and assertions matching computable metrics.
2. Add scripted and LLM recipes in `run_chaos_suite.py` (`SCRIPTED_RECIPES` and `LLM_RECIPES`).
3. Test isolated execution via `--case <case_id> --keep`, then re-run the full `--mode scripted` and `--mode llm` suites.
