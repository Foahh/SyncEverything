# Relay Chaos Suite Report (Track 4 — Scripted)

- Generated: 2026-11-25T05:20:53+0800
- Cases: 8 · Passed: 8 · Failed: 0
- MCP Tools Covered: 9/9 (approve_and_execute_task, check_env_diff, get_dag_snapshot, inspect_sync_status, list_handoffs, propose_worktree_patch, read_evidence_log, read_worktree_file, run_preflight_in_worktree)
- P2P Loopback Gate (`sandbox-dual`): **PASS** (RTT=n/a, duration=5271.7 ms)

| Case | Fault | Result | Tool Recall | Duration (ms) |
|---|---|---|---:|---:|
| `chaos-01-missing-dep` | `missing_dependency` | PASS | 1.00 | 17254.3 |
| `chaos-02-upstream-edit` | `upstream_code_edit` | PASS | 1.00 | 12837.7 |
| `chaos-03-same-line-conflict` | `same_line_conflict` | PASS | 1.00 | 7981.6 |
| `chaos-04-mid-run-disconnect` | `mid_run_disconnect` | PASS | 1.00 | 15844.3 |
| `chaos-05-runtime-exception` | `runtime_exception` | PASS | 1.00 | 8930.2 |
| `chaos-06-corrupted-lock` | `corrupted_env_lock` | PASS | 1.00 | 15567.0 |
| `chaos-07-stale-evidence-trap` | `stale_evidence_trap` | PASS | 1.00 | 23458.5 |
| `chaos-08-duplicate-replay` | `duplicate_task_replay` | PASS | 1.00 | 16795.6 |

## chaos-01-missing-dep — Missing Python Dependency Auto-Healing

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| missing_module_detected | `detected_missing_module` | "tiny_stats" | "tiny_stats" | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| smoke_verified_before_elicitation | `proposal_smoke_verified` | true | true | PASS |
| approval_gate_observed | `approval_required_observed` | true | true | PASS |
| grid_executed_with_evidence | `evidence_fresh_count` | 1 | 1 | PASS |
| task_completed | `task_state_final` | "completed" | "completed" | PASS |

## chaos-02-upstream-edit — Upstream Code Edit Invalidates Host Evidence

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| single_evidence_invalidated | `stale_evidence_count` | 1 | 1 | PASS |
| no_fresh_evidence_left | `evidence_fresh_count` | 0 | 0 | PASS |
| task_record_preserved | `task_state_final` | "completed" | "completed" | PASS |
| stale_log_read_blocked | `stale_read_blocked` | true | true | PASS |

## chaos-03-same-line-conflict — Two-Sided Same-Line Conflict Preserved

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| client_apply_conflict | `conflict_apply_failed` | true | true | PASS |
| client_edit_preserved | `client_edit_preserved` | true | true | PASS |
| host_run_still_verified | `evidence_fresh_count` | 1 | 1 | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |

## chaos-04-mid-run-disconnect — Mid-Run Disconnect Leaves Recoverable Host State

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| state_survives_crash | `crash_state_persisted` | "preflight_running" | "preflight_running" | PASS |
| preflight_resumes | `resume_preflight_passed` | true | true | PASS |
| recovered_run_records_evidence | `evidence_fresh_count` | 1 | 1 | PASS |
| task_completed_after_reconnect | `task_state_final` | "completed" | "completed" | PASS |

## chaos-05-runtime-exception — Runtime Traceback Self-Healing

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| preflight_name_error_detected | `preflight_name_error_detected` | true | true | PASS |
| smoke_verified_after_fix | `proposal_smoke_verified` | true | true | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| grid_metrics_match_fixture | `host_metrics_best_window` | 28 | 28 | PASS |
| task_completed | `task_state_final` | "completed" | "completed" | PASS |

## chaos-06-corrupted-lock — Corrupted Lockfile Recovery

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| lock_parse_failure_detected | `uv_lock_parse_failed` | true | true | PASS |
| repair_smoke_verified | `proposal_smoke_verified` | true | true | PASS |
| host_main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
| task_completed | `task_state_final` | "completed" | "completed" | PASS |

## chaos-07-stale-evidence-trap — Stale Evidence Trap Under Context Ablation

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| stale_read_refused | `stale_read_blocked` | true | true | PASS |
| dag_bundle_quarantines_stale | `dag_bundle_stale_metrics` | 0 | 0 | PASS |
| full_transcript_leaks_stale | `full_transcript_stale_metrics` | 1 | 1 | PASS |
| stale_count_reported | `stale_evidence_count` | 1 | 1 | PASS |

## chaos-08-duplicate-replay — Duplicate Execution Replay Stays Idempotent

| Assertion | Metric | Expected | Actual | Result |
|---|---|---|---|---|
| replay_reports_completed_state | `replay_response` | "execution_started" | "execution_started" | PASS |
| single_evidence_record | `replay_evidence_count` | 1 | 1 | PASS |
| task_stays_completed | `task_state_final` | "completed" | "completed" | PASS |
| main_tree_untouched | `main_worktree_dirty` | false | false | PASS |
