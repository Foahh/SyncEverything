# Chaos Suite — Track 4 Fault-Injection Benchmark

Eight declarative fault-injection cases plus a one-click judge. Every JSON file conforms to the frozen `relay_core::contracts::ChaosCase` schema (`docs/contracts.md` §6); the schema is not extended.

## Coverage

| Case | `injected_fault.kind` | Scenario | Expected tool trace | Core assertions |
|---|---|---|---|---|
| `chaos-01-missing-dep` | `missing_dependency` | Undeclared `tiny_stats` import → offline `uv_spec` wheel repair | preflight → propose → approve | missing module detected, smoke verified, approval gate, 1 fresh evidence, main tree clean |
| `chaos-02-upstream-edit` | `upstream_code_edit` | Laptop commits a new alpha default after the host result | preflight → approve → snapshot → read log | 1 stale evidence, 0 fresh, task record stays `completed`, stale log read refused |
| `chaos-03-same-line-conflict` | `same_line_conflict` | Laptop holds an uncommitted edit on the patched line | preflight → propose → approve | client `git apply` conflicts, client edit preserved, host run still fresh, host tree clean |
| `chaos-04-mid-run-disconnect` | `mid_run_disconnect` | Connection killed mid-preflight, daemon restarted | preflight → approve | state persisted `preflight_running`, preflight resumes, 1 fresh evidence, task `completed` |
| `chaos-05-runtime-exception` | `runtime_exception` | Undefined `DEFAULT_ALPHA` NameError in smoke | preflight → propose → approve | smoke verified after `code_diff`, calibrated `best_window = 28`, task `completed` |
| `chaos-06-corrupted-lock` | `corrupted_env_lock` | Truncated `uv.lock` fails TOML parsing | preflight → propose → approve | parse failure detected, lock repair verified, task `completed` |
| `chaos-07-stale-evidence-trap` | `stale_evidence_trap` | Results requested after an upstream invalidation | snapshot → read log | stale read refused, DAG bundle exposes 0 stale metrics, full transcript exposes 1 |
| `chaos-08-duplicate-replay` | `duplicate_task_replay` | Client replays an approval after a lost response | preflight → approve → snapshot | replay answers `execution_started`, 1 evidence, task stays `completed` |

## One-click judge

```bash
./bench/chaos-cases/judge.sh               # builds the daemon if needed, then runs all cases
uv run python bench/chaos-cases/run_chaos_suite.py   # or call the judge directly
```

Useful flags:

```bash
# one case, keeping its repo/data for inspection
uv run python bench/chaos-cases/run_chaos_suite.py --case chaos-04-mid-run-disconnect --keep

# explicit binary or report directory
uv run python bench/chaos-cases/run_chaos_suite.py --daemon-bin target/debug/relay-daemon --out bench/chaos-cases/results
```

The judge exits non-zero if any assertion fails. It writes `results/chaos-report.json` (machine-readable metrics) and `results/chaos-report.md` (assertion table) on every run.

Latest full run on this workstation: **8/8 PASS** (see `results/chaos-report.md`).

## How a case is executed

1. The fixture is copied into a scratch directory, `git init`-ed, and committed as the baseline (`initial_commit: "HEAD"` is resolved there).
2. The fault is injected and committed, then a `TaskSpec` is submitted through `relay-daemon submit-task`.
3. `relay-daemon mcp-stdio` is driven over JSON-RPC for the tool flow; the judge behaves like an external MCP client, not like the built-in UI agent.
4. Metrics come from four sources: MCP tool responses, the SQLite task store (`--data-dir/relay.db`, read-only), `git status` in the host checkout, and `relay-daemon compose-context --all-strategies --no-sync`.
5. Each `ground_truth_assertions` entry is compared against those metrics with exact equality (floats compared numerically, bools strictly).

No model is required: repairs are deterministic scripted patches, so the suite is a regression test rather than a model benchmark. Track 5 consumes these results for the 8-fault self-healing success-rate figure.

## Determinism and offline policy

- `UV_OFFLINE=1`, `UV_NO_CONFIG=1`, and cleared index variables: no PyPI or machine-local mirror traffic, and no mirror URL leaks into a proposal diff.
- The missing-dependency case installs `vendor/tiny_stats-0.1.0-py3-none-any.whl` from the fixture, so `uv add` and `uv run` stay offline.
- Each case gets a fresh repo and a fresh `--data-dir`; cases never share SQLite state.
- `elapsed_ms` is the only intentionally non-deterministic field and is never asserted.

## Adding a case

1. Add `case_NN_<slug>.json` with a `kind` from `InjectedFaultKind` (`docs/contracts.md` §6) and assertions that reference metrics the judge can compute.
2. Add a recipe in `run_chaos_suite.py` under `RECIPES` keyed by that `kind`, and populate the metric keys used by the JSON.
3. Run the case in isolation with `--case <case_id> --keep`, inspect `resolved` repo/data dirs, then run the full suite.

The suite refuses to start if any frozen fault kind lacks a case JSON.
