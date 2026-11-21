# Chaos Suite Fault-Injection Cases (Track 4)

All JSON cases in this directory conform to `relay_core::contracts::ChaosCase` (see `docs/contracts.md` §6).
Expand from `case_01_missing_dep.json` to cover all 8 fault categories:
1. `missing_dependency`
2. `upstream_code_edit`
3. `same_line_conflict`
4. `mid_run_disconnect`
5. `runtime_exception`
6. `corrupted_env_lock`
7. `stale_evidence_trap`
8. `duplicate_task_replay`
