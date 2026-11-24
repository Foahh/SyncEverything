import contextlib
import csv
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import run_baseline_comparison as baseline
import run_context_ablation as ablation


class BaselineTests(unittest.TestCase):
    def test_incomplete_grid_cannot_win_by_running_only_the_best_window(self):
        grid = {"mode": "grid", "alpha": 0.25, "best_window": 28, "best_mae": 2.4,
                "runs": [{"window": n} for n in (7, 14, 28)]}
        self.assertTrue(baseline.matches_grid(grid, 0.25, 2.4, 28))
        self.assertFalse(baseline.matches_grid({**grid, "runs": [{"window": 28}]}, 0.25, 2.4, 28))

    def test_correction_budget_cannot_be_spent_before_review(self):
        setup = {"task_id": "B1_decision_carryover", "repo": Path("."), "prompt": "run"}
        reply = {"choices": [{"message": {"tool_calls": [{"function": {"name": "list_repo_files"}}]}}]}
        with patch.object(baseline, "call_openai_chat", return_value=(reply, 1)), \
             patch.object(baseline, "execute_workspace_tool", return_value=(False, "[]")), \
             patch.object(baseline, "main_tree_preserved", return_value=(True, [])):
            result = baseline.run_live_case("file_sync_only", setup, Path("daemon"),
                                            {"base_url": "", "model": "fixture", "api_key": None}, 1, 3, 1)
        self.assertEqual(result["llm_turns"], 1)
        self.assertTrue(result["loop_aborted"])

    def test_window_reported_as_a_json_number(self):
        setup = {"task_id": "B3_missing_dep_heal", "expected_mae": 2.16502, "expected_window": 28}
        grid = {"mode": "grid", "alpha": 0.35, "best_window": 28, "best_mae": 2.16502,
                "runs": [{"window": n} for n in (7, 14, 28)]}
        with patch.object(baseline, "main_tree_preserved", return_value=(True, [])):
            for token, expected in [("28", True), ("28.0", True), ("28.5", False), ("128.0", False)]:
                with self.subTest(token=token):
                    passed, _, _ = baseline.check_ground_truth("file_sync_only", setup,
                        {"executed_train_grids": [grid]}, f"best_mae=2.16502, best_window={token}.", None)
                    self.assertEqual(passed, expected)


class AblationTests(unittest.TestCase):
    def test_partial_retry_preserves_other_models_and_uses_identical_inputs(self):
        snapshots = {}
        for name in ("depth_1", "depth_3", "depth_6", "depth_10", "depth_mixed_6"):
            bundles = {s: {"estimated_tokens": 8, "fresh_evidence_count": 1, "stale_evidence_count": 1,
                           "exposed_stale_metrics_count": 0, "content": name + s,
                           "context_bundle_hash": name + "-" + s} for s in ablation.STRATEGIES}
            snapshots[name] = {"snapshot_id": name, "bundles": bundles, "head_commit_sha": "deadbeef",
                               "history_rounds": 1, "fresh_best_maes": ["0.12345"],
                               "stale_best_maes": ["1.12345"], "stale_tokens": [1.12345]}
        models = [{"label": m, "model": m, "api_key": None, "is_local": False, "reasoning_effort": None,
                   "base_url": "example"} for m in ("first", "second")]
        sent = []

        def chat(url, model, key, messages, **kwargs):
            sent.append((model, messages))
            if len(sent) == 1:
                raise TimeoutError("try again")
            return {"choices": [{"message": {"content": "No fresh result."}}]}, 1

        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(ablation, "build_all_snapshots", return_value=snapshots) as build, \
             patch.object(ablation, "find_daemon_binary", return_value=Path("daemon")), \
             patch.object(ablation, "resolve_model_specs", side_effect=[models, models[:1]]), \
             patch.object(ablation, "call_openai_chat", side_effect=chat), \
             contextlib.redirect_stdout(io.StringIO()):
            args = ["ablation", "--out", tmp]
            runs = Path(tmp, "context_ablation_runs.jsonl")
            with patch.object(sys, "argv", args):
                self.assertEqual(ablation.main(), 1)
            before = [json.loads(line) for line in runs.read_text().splitlines()]
            self.assertIsNone(before[0]["stale_hallucination"])
            with patch.object(sys, "argv", [*args, "--retry-errors", "--models", "first"]):
                self.assertEqual(ablation.main(), 0)
            after = [json.loads(line) for line in runs.read_text().splitlines()]
            self.assertEqual(len(after), 36)
            self.assertEqual(after[1:], before[1:])
            self.assertIsNone(after[0]["error"])
            self.assertEqual(sent[0], sent[-1])
            build.assert_called_once()
            with Path(tmp, "context_ablation_scaling.csv").open() as fh:
                scaling = {(r["snapshot_id"], r["strategy"]): r["context_bundle_hash"] for r in csv.DictReader(fh)}
            self.assertTrue(all(scaling[r["snapshot_id"], r["strategy"]] == r["context_bundle_hash"] for r in after))
            matrix = json.loads(Path(tmp, "context_ablation_summary.json").read_text())["matrix"]
            self.assertEqual(len(matrix), 6)
