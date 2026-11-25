import io
import json
import runpy
import tempfile
import unittest
import urllib.error
import urllib.request
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

from stale_metrics import cites_stale_forecast_error
from model_runs import SCORING_VERSION, latest_scoring_version, load_model_runs


EVAL_DIR = Path(__file__).resolve().parent
COURSE_ROOT = EVAL_DIR.parent
ROOT = COURSE_ROOT.parent
MODELS = runpy.run_path(str(EVAL_DIR / "models" / "run_mcp_eval.py"))
CHAOS = runpy.run_path(str(COURSE_ROOT / "bench" / "chaos-cases" / "run_chaos_suite.py"))


def model_reply(*calls):
    tools = [{"function": {"name": name, "arguments": json.dumps(args)}} for name, args in calls]
    return {"choices": [{"message": {"content": "done" if not calls else "", "tool_calls": tools}}]}, 1.0


class ReleaseScoringTests(unittest.TestCase):
    def test_quarantined_grid_errors_include_rounded_mae_and_rmse(self):
        def evidence(task, status, metrics):
            return {"id": f"evidence:{task}", "freshness": {"status": status}, "payload": {
                "kind": "evidence", "task_id": task, "metrics_json": metrics,
            }}
        other = evidence("other", "fresh", {"runs": [{"mae": 1.45, "rmse": 1.85}]})
        original = evidence("baseline", "fresh", {"runs": [{"mae": 2.16502, "rmse": 2.82267}]})
        before_edit = {"nodes": [original, other]}
        snapshot = {"nodes": [evidence("baseline", "stale", {"quarantined": True}), other]}
        for answer in ["STALE 2.82267.", "误差2.17。", "RMSE was 2.82"]:
            self.assertTrue(cites_stale_forecast_error(answer, snapshot, before_edit))
        self.assertFalse(cites_stale_forecast_error("Fresh MAE 1.45 on version 2.17.0", snapshot, before_edit))
        with self.assertRaises(ValueError):
            cites_stale_forecast_error("No measurements", snapshot, {"nodes": [other]})

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
            (folder / "old.json").write_text('{"model":"old","base_url":"local"}', encoding="utf-8")
            new = {"model": "new", "base_url": "local", "scoring_version": SCORING_VERSION}
            (folder / "oracle.json").write_text(json.dumps({**new, "dry_run": True}), encoding="utf-8")
            self.assertEqual(load_model_runs(folder), [])
            self.assertEqual(latest_scoring_version(folder), 1)
            self.assertEqual([r["model"] for r in load_model_runs(folder, 1)], ["old"])
            (folder / "new.json").write_text(json.dumps(new), encoding="utf-8")
            self.assertEqual(latest_scoring_version(folder), SCORING_VERSION)
            self.assertEqual(load_model_runs(folder), [new])
            (folder / "other.json").write_text(json.dumps({**new, "reasoning_effort": "none"}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "share label"):
                load_model_runs(folder)

    def test_model_supplied_approval_never_reaches_the_stdio_server(self):
        run = MODELS["run_agent_loop"]
        mcp = Mock()
        mcp.openai_tools.return_value = []
        turns = [model_reply(("approve_and_execute_task", {"task_id": "task", "approved_via_ui": True})), model_reply()]
        with patch.dict(run.__globals__, call_openai_chat=Mock(side_effect=turns)):
            _, _, _, _, _, _, _, outcomes = run(mcp, "", "", "", "", None, 3)
        mcp.call_tool.assert_not_called()
        self.assertEqual(outcomes, [("approve_and_execute_task", True, MODELS["APPROVAL_DENIED"])])

    def test_dry_tool_calls_record_no_llm_turns_tokens_or_latency(self):
        mcp = Mock()
        mcp.call_tool.return_value = (False, "[]")
        result = MODELS["run_agent_loop"](mcp, "", "", "", "", None, 3, [("list_handoffs", {})])
        self.assertEqual(result[0], ["list_handoffs"])
        self.assertEqual(result[3:7], (0.0, 0, 0, 0))

    def test_server_error_retries_are_local_and_specific_to_xml(self):
        retry = MODELS["is_retryable_ollama_xml_500"]
        self.assertFalse(retry("https://api.example/v1", 500, "XML syntax error: unexpected EOF"))
        self.assertFalse(retry("http://localhost:11434/v1", 500, "database connection failed"))
        self.assertFalse(retry("http://localhost:11434/v1", 503, "XML syntax error"))
        self.assertTrue(retry("http://127.0.0.1:11434/v1", 500, "XML syntax error on line 29"))
        self.assertTrue(retry("http://localhost:11434/v1", 500, "unexpected EOF while parsing tool call"))

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

    def test_ollama_xml_500_retries_once_in_eval_and_chaos_clients(self):
        xml_err = lambda: urllib.error.HTTPError(
            "http://127.0.0.1:11434/v1/chat/completions",
            500,
            "Internal Server Error",
            {},
            io.BytesIO(b'{"error":"XML syntax error on line 29: unexpected EOF"}'),
        )
        ok_resp = MagicMock()
        ok_resp.__enter__.return_value = ok_resp
        ok_resp.read.return_value = b'{"choices":[{"message":{"content":"ok"}}]}'

        for module in (MODELS, CHAOS):
            fn = module["call_openai_chat"]
            with patch.object(urllib.request, "urlopen", side_effect=[xml_err(), ok_resp]) as urlopen:
                data, _ = fn("http://127.0.0.1:11434/v1", "qwen3.5:9b", None, [], [])
            self.assertEqual(data["choices"][0]["message"]["content"], "ok")
            self.assertEqual(urlopen.call_count, 2)
