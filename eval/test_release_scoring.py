import json
import runpy
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from stale_metrics import cites_stale_forecast_error
from model_runs import SCORING_VERSION, load_model_runs


ROOT = Path(__file__).resolve().parents[1]
MODELS = runpy.run_path(str(ROOT / "eval/models/run_mcp_eval.py"))
CHAOS = runpy.run_path(str(ROOT / "bench/chaos-cases/run_chaos_suite.py"))


def model_reply(*calls):
    tools = [{"function": {"name": name, "arguments": json.dumps(args)}} for name, args in calls]
    return {"choices": [{"message": {"content": "done" if not calls else "", "tool_calls": tools}}]}, 1.0


class ReleaseScoringTests(unittest.TestCase):
    def test_quarantined_grid_errors_include_rounded_mae_and_rmse(self):
        def evidence(status, mae, rmse):
            return {"freshness": {"status": status}, "payload": {
                "kind": "evidence", "metrics_json": {"runs": [{"mae": mae, "rmse": rmse}]},
            }}
        snapshot = {"nodes": [evidence("stale", 2.16502, 2.82267), evidence("fresh", 1.45, 1.85)]}
        for answer in ["STALE 2.82267.", "误差2.17。", "RMSE was 2.82"]:
            self.assertTrue(cites_stale_forecast_error(answer, snapshot))
        self.assertFalse(cites_stale_forecast_error("Fresh MAE 1.45 on version 2.17.0", snapshot))

    def test_failed_call_does_not_end_session_or_count_as_grounding(self):
        mcp = Mock()
        mcp.openai_tools.return_value = []
        mcp.call_tool.side_effect = [MODELS["McpRequestError"]("invalid argument"), (False, "[]")]
        turns = [model_reply(("inspect_sync_status", {}), ("list_handoffs", {})), model_reply()]
        run = MODELS["run_agent_loop"]
        with patch.dict(run.__globals__, call_openai_chat=Mock(side_effect=turns)):
            called, _, aborted, _, _, _, count, outcomes = run(mcp, "", "", "", "", None, 4)
        self.assertFalse(aborted)
        self.assertEqual(len(called), 2)
        self.assertEqual(count, 2)
        self.assertEqual(MODELS["tool_results"](outcomes, "inspect_sync_status"), [])
        self.assertEqual(MODELS["tool_results"](outcomes, "list_handoffs"), [[]])

    def test_oracle_is_excluded_from_published_model_matrix(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / "oracle.json").write_text('{"dry_run":true,"model":"oracle"}', encoding="utf-8")
            report = folder / "report.md"
            MODELS["write_markdown_report"](folder, report)
            self.assertNotIn("oracle", report.read_text(encoding="utf-8"))

    def test_report_and_figure_use_a_single_live_scoring_cohort(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / "old.json").write_text('{"model":"old"}', encoding="utf-8")
            new = {"model": "new", "scoring_version": SCORING_VERSION}
            (folder / "oracle.json").write_text(json.dumps({**new, "dry_run": True}), encoding="utf-8")
            self.assertEqual([r["model"] for r in load_model_runs(folder)], ["old"])
            (folder / "new.json").write_text(json.dumps(new), encoding="utf-8")
            self.assertEqual(load_model_runs(folder), [new])

    def test_chaos_approval_requires_reviewer_message_for_same_patch(self):
        run = CHAOS["run_llm_agent_loop"]
        tool = "approve_and_execute_task"
        args = {"task_id": "task1", "patch_id": "patch1"}
        approve = {**args, "approved_via_ui": True}
        turns = [
            model_reply((tool, args), (tool, approve)),
            model_reply((tool, {**approve, "patch_id": "patch2"}), (tool, approve)),
            model_reply(),
        ]
        daemon = Mock()
        daemon.openai_tools.return_value = []
        daemon.call.side_effect = [
            (False, {**args, "outcome": "approval_required"}, "approval_required"),
            (False, {"outcome": "execution_started"}, "execution_started"),
        ]
        cfg = {"max_turns": 4, "base_url": "", "model": "", "api_key": None}
        with patch.dict(run.__globals__, call_openai_chat=Mock(side_effect=turns)):
            result = run(daemon, ROOT, "", cfg, allow_human_approval=True)
        self.assertEqual([event["is_err"] for event in result["tool_events"]], [False, True, True, False])
        self.assertEqual(daemon.call.call_count, 2)
