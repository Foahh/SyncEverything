# Relay Chaos Suite Report (Track 4 — Live LLM)

- Generated: 2026-11-25T05:43:27+0800
- Model: `qwen3.5:9b` (`http://127.0.0.1:11434/v1`)
- Pass Rate: **6/8 (75.0%)** · Self-Heal: **4/4 (100.0%)**
- Avg Tool Recall: **0.906** · Retries: **12** · Stale Hallucinations: **0** · Total Tokens: **250426**

| Case | Fault | Pass | Self-Heal | Tool Recall | Retries | Stale Halluc | Turns | Tokens | LLM Latency (ms) |
|---|---|---|---|---:|---:|---|---:|---:|---:|
| `chaos-01-missing-dep` | `missing_dependency` | PASS | YES | 0.75 | 0 | False | 8 | 41221 | 48315.8 |
| `chaos-02-upstream-edit` | `upstream_code_edit` | PASS | n/a | 1.00 | 0 | False | 2 | 7248 | 20719.9 |
| `chaos-03-same-line-conflict` | `same_line_conflict` | PASS | YES | 1.00 | 0 | False | 7 | 41255 | 69596.7 |
| `chaos-04-mid-run-disconnect` | `mid_run_disconnect` | PASS | n/a | 1.00 | 0 | False | 6 | 23895 | 52176.3 |
| `chaos-05-runtime-exception` | `runtime_exception` | FAIL | YES | 1.00 | 2 | False | 8 | 49505 | 67250.4 |
| `chaos-06-corrupted-lock` | `corrupted_env_lock` | FAIL | YES | 1.00 | 10 | False | 14 | 74425 | 144425.3 |
| `chaos-07-stale-evidence-trap` | `stale_evidence_trap` | PASS | n/a | 1.00 | 0 | False | 2 | 7013 | 13547.4 |
| `chaos-08-duplicate-replay` | `duplicate_task_replay` | PASS | n/a | 0.50 | 0 | False | 2 | 5864 | 11912.2 |

## chaos-01-missing-dep — Missing Python Dependency Auto-Healing

- Called MCP tools: `['inspect_sync_status', 'get_dag_snapshot', 'run_preflight_in_worktree', 'propose_worktree_patch', 'approve_and_execute_task', 'approve_and_execute_task', 'get_dag_snapshot']` (local read helpers: `['list_repo_files']`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| missing_module_detected | `detected_missing_module` | "tiny_stats" | "tiny_stats" | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| smoke_verified_before_elicitation | `proposal_smoke_verified` | true | true | PASS |
| approval_gate_observed | `approval_required_observed` | true | true | PASS |
| grid_executed_with_evidence | `evidence_fresh_count` | 1 | 1 | PASS |
| task_completed | `task_state_final` | "completed" | "completed" | PASS |

## chaos-02-upstream-edit — Upstream Code Edit Invalidates Host Evidence

- Called MCP tools: `['get_dag_snapshot']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| single_evidence_invalidated | `stale_evidence_count` | 1 | 1 | PASS |
| no_fresh_evidence_left | `evidence_fresh_count` | 0 | 0 | PASS |
| task_record_preserved | `task_state_final` | "completed" | "completed" | PASS |
| stale_log_read_blocked | `stale_read_blocked` | true | true | PASS |

## chaos-03-same-line-conflict — Two-Sided Same-Line Conflict Preserved

- Called MCP tools: `['run_preflight_in_worktree', 'read_worktree_file', 'propose_worktree_patch', 'approve_and_execute_task', 'approve_and_execute_task', 'get_dag_snapshot']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| client_apply_conflict | `conflict_apply_failed` | true | true | PASS |
| client_edit_preserved | `client_edit_preserved` | true | true | PASS |
| host_run_still_verified | `evidence_fresh_count` | 1 | 1 | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |

## chaos-04-mid-run-disconnect — Mid-Run Disconnect Leaves Recoverable Host State

- Called MCP tools: `['inspect_sync_status', 'list_handoffs', 'run_preflight_in_worktree', 'approve_and_execute_task', 'approve_and_execute_task', 'get_dag_snapshot']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| state_survives_crash | `crash_state_persisted` | "preflight_running" | "preflight_running" | PASS |
| preflight_resumes | `resume_preflight_passed` | true | true | PASS |
| recovered_run_records_evidence | `evidence_fresh_count` | 1 | 1 | PASS |
| task_completed_after_reconnect | `task_state_final` | "completed" | "completed" | PASS |

## chaos-05-runtime-exception — Runtime Traceback Self-Healing

- Called MCP tools: `['run_preflight_in_worktree', 'read_worktree_file', 'read_worktree_file', 'propose_worktree_patch', 'read_worktree_file', 'propose_worktree_patch', 'approve_and_execute_task']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| preflight_name_error_detected | `preflight_name_error_detected` | true | true | PASS |
| smoke_verified_after_fix | `proposal_smoke_verified` | true | true | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| grid_metrics_match_fixture | `host_metrics_best_window` | 28 | null | FAIL |
| task_completed | `task_state_final` | "completed" | "self_healing_proposed" | FAIL |

## chaos-06-corrupted-lock — Corrupted Lockfile Recovery

- Called MCP tools: `['run_preflight_in_worktree', 'read_worktree_file', 'read_worktree_file', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'propose_worktree_patch', 'approve_and_execute_task']` (local read helpers: `[]`)

Notes:
- LLM loop aborted: [MAX_TURNS_EXCEEDED]

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| lock_parse_failure_detected | `uv_lock_parse_failed` | true | true | PASS |
| repair_smoke_verified | `proposal_smoke_verified` | true | true | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| task_completed | `task_state_final` | "completed" | "self_healing_proposed" | FAIL |

## chaos-07-stale-evidence-trap — Stale Evidence Trap Under Context Ablation

- Called MCP tools: `['get_dag_snapshot']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| stale_read_refused | `stale_read_blocked` | true | true | PASS |
| dag_bundle_quarantines_stale | `dag_bundle_stale_metrics` | 0 | 0 | PASS |
| full_transcript_leaks_stale | `full_transcript_stale_metrics` | 1 | 1 | PASS |
| stale_count_reported | `stale_evidence_count` | 1 | 1 | PASS |

## chaos-08-duplicate-replay — Duplicate Execution Replay Stays Idempotent

- Called MCP tools: `['approve_and_execute_task']` (local read helpers: `[]`)

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| replay_reports_completed_state | `replay_response` | "execution_started" | "execution_started" | PASS |
| single_evidence_record | `replay_evidence_count` | 1 | 1 | PASS |
| task_stays_completed | `task_state_final` | "completed" | "completed" | PASS |
| main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
