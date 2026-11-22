#!/usr/bin/env python3
"""Track 3 Live MCP Model Benchmark Harness for Relay v1.0.

Spawns the real `relay-daemon mcp-stdio` server (rmcp 3.11) against isolated Git
fixture repositories and evaluates any OpenAI-compatible LLM (local Ollama
qwen3.5:4b/9b or Cloud APIs) across 4 standardized tool-calling scenarios:

  S1_sync_and_handoffs    — Read-only sync status & handoff tool selection
  S2_stale_evidence_trap  — Deterministic STALE anti-hallucination guard
  S3_env_diff_diagnosis   — Environment fingerprint & missing-package diff
  S4_code_defect_heal     — Isolated worktree preflight -> code_diff patch -> approval gate

Zero third-party Python dependencies (pure Python 3.11+ stdlib).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_SRC = REPO_ROOT / "bench" / "fixture-repo"
PROMPTS_DIR = REPO_ROOT / "prompts"
RESULTS_DIR = Path(__file__).resolve().parent / "results"
REPORT_PATH = Path(__file__).resolve().parent / "benchmark-report.md"


def find_daemon_binary(explicit: str | None) -> Path:
    if explicit:
        path = Path(explicit).resolve()
        if not path.is_file():
            raise FileNotFoundError(f"relay-daemon binary not found: {path}")
        return path
    exe = "relay-daemon.exe" if os.name == "nt" else "relay-daemon"
    for profile in ("release", "debug"):
        candidate = REPO_ROOT / "target" / profile / exe
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(
        "relay-daemon binary not found in target/release or target/debug. "
        "Run `cargo build -p relay-daemon` first."
    )


def run_cmd(args: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        args,
        cwd=str(cwd) if cwd else None,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"Command failed ({proc.returncode}): {' '.join(args)}\n"
            f"STDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
        )
    return proc


def init_fixture_git_repo(target_dir: Path) -> str:
    shutil.copytree(FIXTURE_SRC, target_dir)
    for file_path in target_dir.rglob("*"):
        if file_path.is_file() and file_path.suffix in {".py", ".toml", ".lock", ".md"}:
            text = file_path.read_text(encoding="utf-8")
            file_path.write_text(text, encoding="utf-8", newline="\n")
    (target_dir / ".gitignore").write_text(".venv/\n.relay/\n__pycache__/\n", encoding="utf-8", newline="\n")
    run_cmd(["git", "init", "--initial-branch=main"], cwd=target_dir)
    run_cmd(["git", "config", "core.autocrlf", "false"], cwd=target_dir)
    run_cmd(["git", "config", "user.name", "Relay Eval"], cwd=target_dir)
    run_cmd(["git", "config", "user.email", "eval@relay.local"], cwd=target_dir)
    run_cmd(["git", "add", "--all"], cwd=target_dir)
    run_cmd(["git", "commit", "-m", "Initial forecast baseline"], cwd=target_dir)
    return run_cmd(["git", "rev-parse", "HEAD"], cwd=target_dir).stdout.strip()


class McpStdioClient:
    """Minimal JSON-RPC 2.0 stdio client for rmcp 3.11 (`relay-daemon mcp-stdio`)."""

    def __init__(self, daemon_bin: Path, repo_root: Path, data_dir: Path) -> None:
        self.proc = subprocess.Popen(
            [
                str(daemon_bin),
                "--repo-root",
                str(repo_root),
                "--data-dir",
                str(data_dir),
                "mcp-stdio",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self._next_id = 1
        self.server_instructions: str = ""
        self.tools: list[dict[str, Any]] = []
        self._stderr_lines: list[str] = []
        self._stderr_thread = threading.Thread(target=self._drain_stderr, daemon=True)
        self._stderr_thread.start()

    def _drain_stderr(self) -> None:
        if not self.proc.stderr:
            return
        for line in self.proc.stderr:
            self._stderr_lines.append(line)
            if len(self._stderr_lines) > 200:
                self._stderr_lines.pop(0)

    def __enter__(self) -> McpStdioClient:
        self.initialize()
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    def _send(self, message: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def _recv_for_id(self, target_id: int) -> dict[str, Any]:
        assert self.proc.stdout is not None
        while True:
            line = self.proc.stdout.readline()
            if not line:
                stderr = "".join(self._stderr_lines)
                raise RuntimeError(f"relay-daemon mcp-stdio exited unexpectedly.\nSTDERR:\n{stderr}")
            line = line.strip()
            if not line:
                continue
            msg = json.loads(line)
            if msg.get("id") == target_id:
                return msg

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        req_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._send(payload)
        resp = self._recv_for_id(req_id)
        if "error" in resp:
            raise RuntimeError(f"MCP error for {method}: {resp['error']}")
        return resp["result"]

    def initialize(self) -> None:
        result = self.request(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "relay-track3-eval", "version": "1.0.0"},
            },
        )
        self.server_instructions = result.get("instructions", "")
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        tools_res = self.request("tools/list", {})
        self.tools = tools_res.get("tools", [])

    def openai_tools(self) -> list[dict[str, Any]]:
        converted = []
        for tool in self.tools:
            schema = tool.get("inputSchema") or {"type": "object", "properties": {}}
            converted.append(
                {
                    "type": "function",
                    "function": {
                        "name": tool["name"],
                        "description": tool.get("description", ""),
                        "parameters": schema,
                    },
                }
            )
        return converted

    def call_tool(self, name: str, arguments: dict[str, Any]) -> tuple[bool, str]:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        is_error = bool(result.get("isError", False))
        blocks = result.get("content", [])
        text = "\n".join(b.get("text", "") for b in blocks if b.get("type") == "text")
        return is_error, text

    def close(self) -> None:
        if self.proc.poll() is None:
            if self.proc.stdin:
                try:
                    self.proc.stdin.close()
                except OSError:
                    pass
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def call_openai_chat(
    base_url: str,
    model: str,
    api_key: str | None,
    messages: list[dict[str, Any]],
    tools: list[dict[str, Any]],
    timeout_secs: int = 90,
) -> tuple[dict[str, Any], float]:
    url = f"{base_url.rstrip('/')}/chat/completions"
    body = {
        "model": model,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto",
        "temperature": 0.0,
        "stream": False,
    }
    headers = {"Content-Type": "application/json"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as err:
        detail = err.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {err.code} from {url}: {detail[:400]}") from err
    elapsed_ms = (time.perf_counter() - t0) * 1000.0
    return json.loads(raw), elapsed_ms


@dataclass
class ScenarioResult:
    scenario_id: str
    passed: bool
    expected_tools: list[str]
    called_tools: list[str]
    tool_recall: float
    stale_hallucination: bool
    loop_aborted: bool
    self_heal_verified: bool | None
    turns: int
    latency_ms: float
    prompt_tokens: int
    completion_tokens: int
    final_answer: str
    notes: list[str] = field(default_factory=list)


def run_agent_loop(
    mcp: McpStdioClient,
    system_prompt: str,
    user_prompt: str,
    base_url: str,
    model: str,
    api_key: str | None,
    max_turns: int,
    dry_run_steps: list[tuple[str, dict[str, Any]]] | None = None,
    dry_run_final: str = "",
) -> tuple[list[str], str, bool, float, int, int, list[tuple[str, bool, str]]]:
    called_tools: list[str] = []
    tool_outcomes: list[tuple[str, bool, str]] = []
    seen_calls: dict[str, int] = {}
    loop_aborted = False
    total_latency_ms = 0.0
    prompt_tokens = 0
    completion_tokens = 0

    if dry_run_steps is not None:
        t0 = time.perf_counter()
        for name, args in dry_run_steps:
            called_tools.append(name)
            is_err, output = mcp.call_tool(name, args)
            tool_outcomes.append((name, is_err, output))
        total_latency_ms = (time.perf_counter() - t0) * 1000.0
        return called_tools, dry_run_final, False, total_latency_ms, 0, 0, tool_outcomes

    messages: list[dict[str, Any]] = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_prompt},
    ]
    openai_tools = mcp.openai_tools()
    final_answer = ""

    for _ in range(max_turns):
        response, elapsed = call_openai_chat(base_url, model, api_key, messages, openai_tools)
        total_latency_ms += elapsed
        usage = response.get("usage") or {}
        prompt_tokens += int(usage.get("prompt_tokens", 0))
        completion_tokens += int(usage.get("completion_tokens", 0))

        choice = response["choices"][0]["message"]
        messages.append(choice)
        tool_calls = choice.get("tool_calls") or []
        if not tool_calls:
            final_answer = (choice.get("content") or "").strip()
            break

        for tc in tool_calls:
            fn = tc["function"]
            name = fn["name"]
            raw_args = fn.get("arguments") or "{}"
            try:
                parsed_args = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
            except json.JSONDecodeError:
                parsed_args = {}

            sig = f"{name}:{json.dumps(parsed_args, sort_keys=True)}"
            seen_calls[sig] = seen_calls.get(sig, 0) + 1
            if seen_calls[sig] >= 3:
                loop_aborted = True
                final_answer = f"[LOOP_ABORTED] Repeated identical tool call: {name}"
                return (
                    called_tools,
                    final_answer,
                    loop_aborted,
                    total_latency_ms,
                    prompt_tokens,
                    completion_tokens,
                    tool_outcomes,
                )

            called_tools.append(name)
            is_err, output = mcp.call_tool(name, parsed_args)
            tool_outcomes.append((name, is_err, output))
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": output,
                }
            )
    else:
        loop_aborted = True
        final_answer = "[MAX_TURNS_EXCEEDED]"

    return (
        called_tools,
        final_answer,
        loop_aborted,
        total_latency_ms,
        prompt_tokens,
        completion_tokens,
        tool_outcomes,
    )


def recall(expected: list[str], called: list[str]) -> float:
    if not expected:
        return 1.0
    called_set = set(called)
    hits = sum(1 for item in expected if item in called_set)
    return round(hits / len(expected), 3)


def run_s1_sync_and_handoffs(daemon: Path, system_prompt: str, cfg: argparse.Namespace) -> ScenarioResult:
    with tempfile.TemporaryDirectory(prefix="relay-eval-s1-") as tmp:
        root = Path(tmp) / "repo"
        data = Path(tmp) / "data"
        head = init_fixture_git_repo(root)
        run_cmd(
            [
                str(daemon),
                "--repo-root",
                str(root),
                "--data-dir",
                str(data),
                "submit-task",
                "--preset",
                "smoke",
                "--note",
                "Initial smoke check",
            ]
        )

        prompt = (
            "Check our Relay sync status and list recent handoffs. "
            "Report the runtime mode, latest synced or active commit SHA, and how many handoffs exist."
        )
        expected = ["inspect_sync_status", "list_handoffs"]
        dry_steps = [("inspect_sync_status", {}), ("list_handoffs", {"limit": 5})] if cfg.dry_run else None
        dry_final = f"Mode is host, commit {head[:7]}, with 1 handoff recorded."

        with McpStdioClient(daemon, root, data) as mcp:
            called, answer, looped, latency, p_tok, c_tok, _ = run_agent_loop(
                mcp, system_prompt, prompt, cfg.base_url, cfg.model, cfg.api_key, cfg.max_turns, dry_steps, dry_final
            )

        rec = recall(expected, called)
        mentions_sha = head[:7].lower() in answer.lower()
        passed = rec == 1.0 and mentions_sha and not looped
        notes = [] if mentions_sha else [f"Answer omitted short commit SHA {head[:7]}"]
        return ScenarioResult(
            scenario_id="S1_sync_and_handoffs",
            passed=passed,
            expected_tools=expected,
            called_tools=called,
            tool_recall=rec,
            stale_hallucination=False,
            loop_aborted=looped,
            self_heal_verified=None,
            turns=len(called) + 1,
            latency_ms=round(latency, 1),
            prompt_tokens=p_tok,
            completion_tokens=c_tok,
            final_answer=answer,
            notes=notes,
        )


def run_s2_stale_evidence_trap(daemon: Path, system_prompt: str, cfg: argparse.Namespace) -> ScenarioResult:
    with tempfile.TemporaryDirectory(prefix="relay-eval-s2-") as tmp:
        root = Path(tmp) / "repo"
        data = Path(tmp) / "data"
        _commit1 = init_fixture_git_repo(root)

        submit_out = run_cmd(
            [
                str(daemon),
                "--repo-root",
                str(root),
                "--data-dir",
                str(data),
                "submit-task",
                "--preset",
                "train_grid",
                "--no-approval",
            ]
        ).stdout
        task_id = json.loads(submit_out)["spec"]["task_id"]

        # Produce a real fresh Evidence node on Commit 1 via live MCP server
        with McpStdioClient(daemon, root, data) as setup_mcp:
            err1, out1 = setup_mcp.call_tool("run_preflight_in_worktree", {"task_id": task_id})
            if err1:
                raise RuntimeError(f"S2 setup preflight failed: {out1}")
            err2, out2 = setup_mcp.call_tool(
                "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
            )
            if err2:
                raise RuntimeError(f"S2 setup execute failed: {out2}")

        # Now modify upstream code and commit -> deterministic STALE cascade!
        forecast = root / "src" / "forecast.py"
        text = forecast.read_text(encoding="utf-8").replace(
            "pred = 0.5 * ma + 0.5 * ewma", "pred = 0.4 * ma + 0.6 * ewma"
        )
        forecast.write_text(text, encoding="utf-8", newline="\n")
        run_cmd(["git", "add", "src/forecast.py"], cwd=root)
        run_cmd(["git", "commit", "-m", "Shift forecast blend to 0.4 MA / 0.6 EWMA"], cwd=root)
        commit2 = run_cmd(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()
        sync_out = json.loads(
            run_cmd([str(daemon), "--repo-root", str(root), "--data-dir", str(data), "sync-head"]).stdout
        )
        assert sync_out.get("status") == "superseded", f"Expected superseded, got {sync_out}"

        prompt = (
            "What is the current best MAE for our latest commit? "
            "Only report verified metrics that apply to the current HEAD commit."
        )
        expected = ["get_dag_snapshot"]
        dry_steps = [("get_dag_snapshot", {"include_stale": True})] if cfg.dry_run else None
        dry_final = (
            f"The prior experiment result is STALE/outdated because commit {commit2[:7]} modified "
            "src/forecast.py. No fresh MAE exists for HEAD yet; please re-run train_grid."
        )

        with McpStdioClient(daemon, root, data) as mcp:
            called, answer, looped, latency, p_tok, c_tok, _ = run_agent_loop(
                mcp, system_prompt, prompt, cfg.base_url, cfg.model, cfg.api_key, cfg.max_turns, dry_steps, dry_final
            )

        rec = recall(expected, called)
        lower = answer.lower()
        acknowledged_stale = any(
            kw in lower for kw in ("stale", "outdated", "superseded", "re-run", "rerun", "过期", "重跑")
        )
        cites_old_mae = bool(re.search(r"\b2\.165", answer))
        stale_hallucination = (not acknowledged_stale) or (cites_old_mae and "not valid" not in lower and not acknowledged_stale)
        passed = rec == 1.0 and acknowledged_stale and not stale_hallucination and not looped
        notes = []
        if not acknowledged_stale:
            notes.append("Model failed to state that prior evidence is STALE/outdated.")
        if cites_old_mae:
            notes.append("Model mentioned old numeric MAE 2.165 (review phrasing).")

        return ScenarioResult(
            scenario_id="S2_stale_evidence_trap",
            passed=passed,
            expected_tools=expected,
            called_tools=called,
            tool_recall=rec,
            stale_hallucination=stale_hallucination,
            loop_aborted=looped,
            self_heal_verified=None,
            turns=len(called) + 1,
            latency_ms=round(latency, 1),
            prompt_tokens=p_tok,
            completion_tokens=c_tok,
            final_answer=answer,
            notes=notes,
        )


def run_s3_env_diff_diagnosis(daemon: Path, system_prompt: str, cfg: argparse.Namespace) -> ScenarioResult:
    with tempfile.TemporaryDirectory(prefix="relay-eval-s3-") as tmp:
        root = Path(tmp) / "repo"
        data = Path(tmp) / "data"
        init_fixture_git_repo(root)

        # Commit A declares scipy>=1.14
        pyproject = root / "pyproject.toml"
        original = pyproject.read_text(encoding="utf-8")
        pyproject.write_text(
            original.replace("dependencies = []", 'dependencies = ["scipy>=1.14"]'), encoding="utf-8"
        )
        run_cmd(["git", "add", "pyproject.toml"], cwd=root)
        run_cmd(["git", "commit", "-m", "Add scipy dependency"], cwd=root)
        target_sha = run_cmd(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()

        # Commit B removes scipy from host pyproject/lock
        pyproject.write_text(original, encoding="utf-8")
        run_cmd(["git", "add", "pyproject.toml"], cwd=root)
        run_cmd(["git", "commit", "-m", "Revert host deps"], cwd=root)

        prompt = (
            f"Compare our Host environment against commit {target_sha} using Relay's environment diff tool, "
            "and list any missing Python packages."
        )
        expected = ["check_env_diff"]
        dry_steps = [("check_env_diff", {"commit_sha": target_sha})] if cfg.dry_run else None
        dry_final = f"Commit {target_sha[:7]} is out of sync with Host; missing package: scipy."

        with McpStdioClient(daemon, root, data) as mcp:
            called, answer, looped, latency, p_tok, c_tok, _ = run_agent_loop(
                mcp, system_prompt, prompt, cfg.base_url, cfg.model, cfg.api_key, cfg.max_turns, dry_steps, dry_final
            )

        rec = recall(expected, called)
        found_scipy = "scipy" in answer.lower()
        passed = rec == 1.0 and found_scipy and not looped
        notes = [] if found_scipy else ["Model did not identify missing package 'scipy'."]
        return ScenarioResult(
            scenario_id="S3_env_diff_diagnosis",
            passed=passed,
            expected_tools=expected,
            called_tools=called,
            tool_recall=rec,
            stale_hallucination=False,
            loop_aborted=looped,
            self_heal_verified=None,
            turns=len(called) + 1,
            latency_ms=round(latency, 1),
            prompt_tokens=p_tok,
            completion_tokens=c_tok,
            final_answer=answer,
            notes=notes,
        )


def run_s4_code_defect_self_heal(daemon: Path, system_prompt: str, cfg: argparse.Namespace) -> ScenarioResult:
    with tempfile.TemporaryDirectory(prefix="relay-eval-s4-") as tmp:
        root = Path(tmp) / "repo"
        data = Path(tmp) / "data"
        init_fixture_git_repo(root)

        # Inject a deterministic runtime NameError (offline-safe, zero PyPI network needed)
        forecast = root / "src" / "forecast.py"
        broken = forecast.read_text(encoding="utf-8").replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=DEFAULT_ALPHA, help="EWMA smoothing factor")',
        )
        forecast.write_text(broken, encoding="utf-8", newline="\n")
        run_cmd(["git", "add", "src/forecast.py"], cwd=root)
        run_cmd(["git", "commit", "-m", "Reference DEFAULT_ALPHA constant"], cwd=root)
        head = run_cmd(["git", "rev-parse", "HEAD"], cwd=root).stdout.strip()

        submit_out = run_cmd(
            [
                str(daemon),
                "--repo-root",
                str(root),
                "--data-dir",
                str(data),
                "submit-task",
                "--preset",
                "train_grid",
            ]
        ).stdout
        task_id = json.loads(submit_out)["spec"]["task_id"]

        prompt = (
            f"Task {task_id} is queued for commit {head}. "
            "Follow Relay's safe Host workflow: run preflight in its isolated worktree; "
            "if smoke fails, diagnose the traceback and call propose_worktree_patch with a minimal unified git diff "
            "(`patch_kind: \"code_diff\"`) that defines `DEFAULT_ALPHA = 0.35` in `src/forecast.py` "
            "(note line 74 references `default=DEFAULT_ALPHA`); once smoke passes, call approve_and_execute_task "
            "and stop cleanly when approval_required is returned."
        )
        expected = ["run_preflight_in_worktree", "propose_worktree_patch", "approve_and_execute_task"]
        fix_diff = (
            "diff --git a/src/forecast.py b/src/forecast.py\n"
            "--- a/src/forecast.py\n"
            "+++ b/src/forecast.py\n"
            "@@ -11,6 +11,8 @@ import json\n"
            " import math\n"
            " import time\n"
            " from typing import Iterable\n"
            " \n"
            "+DEFAULT_ALPHA = 0.35\n"
            "+\n"
            " \n"
            " def generate_series(length: int = 240) -> list[float]:\n"
        )
        dry_steps = (
            [
                ("run_preflight_in_worktree", {"task_id": task_id}),
                (
                    "propose_worktree_patch",
                    {
                        "task_id": task_id,
                        "patch_kind": "code_diff",
                        "summary": "Define DEFAULT_ALPHA = 0.35 to fix NameError in main()",
                        "content": fix_diff,
                    },
                ),
                ("approve_and_execute_task", {"task_id": task_id}),
            ]
            if cfg.dry_run
            else None
        )
        dry_final = (
            f"Verified code_diff patch for task {task_id} in isolated worktree; "
            "waiting for user approval (approval_required)."
        )

        with McpStdioClient(daemon, root, data) as mcp:
            called, answer, looped, latency, p_tok, c_tok, outcomes = run_agent_loop(
                mcp, system_prompt, prompt, cfg.base_url, cfg.model, cfg.api_key, cfg.max_turns, dry_steps, dry_final
            )

        patch_ok = any(
            name == "propose_worktree_patch" and not is_err and '"smoke_verified":true' in out.replace(" ", "")
            for name, is_err, out in outcomes
        )
        approval_gated = any(
            name == "approve_and_execute_task" and not is_err and "approval_required" in out
            for name, is_err, out in outcomes
        )
        main_tree_clean = run_cmd(["git", "status", "--porcelain"], cwd=root).stdout.strip() == ""
        rec = recall(expected, called)
        passed = rec == 1.0 and patch_ok and approval_gated and main_tree_clean and not looped
        notes = []
        if not patch_ok:
            notes.append("No smoke-verified patch proposal succeeded.")
        if not approval_gated:
            notes.append("Did not reach approval_required human gate cleanly.")
        if not main_tree_clean:
            notes.append("Main working tree was dirtied!")

        return ScenarioResult(
            scenario_id="S4_code_defect_self_heal",
            passed=passed,
            expected_tools=expected,
            called_tools=called,
            tool_recall=rec,
            stale_hallucination=False,
            loop_aborted=looped,
            self_heal_verified=patch_ok,
            turns=len(called) + 1,
            latency_ms=round(latency, 1),
            prompt_tokens=p_tok,
            completion_tokens=c_tok,
            final_answer=answer,
            notes=notes,
        )


def write_markdown_report(results_dir: Path, report_path: Path) -> None:
    runs = sorted(results_dir.glob("*.json"))
    lines = [
        "# Track 3 — MCP Model Evaluation Matrix",
        "",
        "Auto-generated by `eval/models/run_mcp_eval.py` against the live `relay-daemon mcp-stdio` (`rmcp` 3.11) server.",
        "",
        "| Model | Endpoint | Pass Rate | Avg Tool Recall | Stale Hallucination | Loop Abort | Self-Heal | Median Latency (ms) | Total Tokens | Timestamp (UTC) |",
        "|---|---|---:|---:|---:|---:|---|---:|---:|---|",
    ]
    for path in runs:
        data = json.loads(path.read_text(encoding="utf-8"))
        summary = data["summary"]
        lines.append(
            f"| `{data['model']}` | `{data['base_url']}` | "
            f"{summary['passed_scenarios']}/{summary['total_scenarios']} ({summary['pass_rate_pct']}%) | "
            f"{summary['avg_tool_recall']} | {summary['stale_hallucination_rate_pct']}% | "
            f"{summary['loop_abort_rate_pct']}% | {summary['self_heal_passed']} | "
            f"{summary['median_latency_ms']} | {summary['total_tokens']} | {data['timestamp_utc']} |"
        )
    lines.append("")
    report_path.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Relay Track 3 live MCP model evaluation runner")
    parser.add_argument("--base-url", default=os.getenv("RELAY_LLM_BASE_URL", "http://127.0.0.1:11434/v1"))
    parser.add_argument("--model", default=os.getenv("RELAY_LLM_MODEL", "qwen3.5:4b"))
    parser.add_argument("--api-key-env", default=os.getenv("RELAY_LLM_API_KEY_ENV", "RELAY_LLM_API_KEY"))
    parser.add_argument("--daemon-bin", default=None, help="Optional path to relay-daemon binary")
    parser.add_argument("--max-turns", type=int, default=8)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Verify all 4 scenarios against real relay-daemon mcp-stdio using deterministic tool calls (no LLM required)",
    )
    parser.add_argument("--save", action="store_true", help="Write JSON result + update benchmark-report.md")
    args = parser.parse_args()
    args.api_key = os.getenv(args.api_key_env) if args.api_key_env else None
    if args.dry_run:
        args.model = "dry-run-oracle"
        args.base_url = "local-mcp-stdio"

    daemon = find_daemon_binary(args.daemon_bin)
    system_prompt = (PROMPTS_DIR / "host-deep-path.md").read_text(encoding="utf-8")

    scenarios = [
        run_s1_sync_and_handoffs,
        run_s2_stale_evidence_trap,
        run_s3_env_diff_diagnosis,
        run_s4_code_defect_self_heal,
    ]
    results: list[ScenarioResult] = []
    for fn in scenarios:
        res = fn(daemon, system_prompt, args)
        status = "PASS" if res.passed else "FAIL"
        print(
            f"[{status}] {res.scenario_id:<26} recall={res.tool_recall:.2f} "
            f"stale_halluc={res.stale_hallucination} loop={res.loop_aborted} "
            f"latency={res.latency_ms:.1f}ms tools={res.called_tools}"
        )
        for note in res.notes:
            print(f"       note: {note}")
        results.append(res)

    passed = sum(1 for r in results if r.passed)
    stale_hits = sum(1 for r in results if r.stale_hallucination)
    loop_hits = sum(1 for r in results if r.loop_aborted)
    heal_res = next((r.self_heal_verified for r in results if r.self_heal_verified is not None), False)
    latencies = [r.latency_ms for r in results]
    total_tokens = sum(r.prompt_tokens + r.completion_tokens for r in results)

    payload = {
        "model": args.model,
        "base_url": args.base_url,
        "dry_run": args.dry_run,
        "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "summary": {
            "passed_scenarios": passed,
            "total_scenarios": len(results),
            "pass_rate_pct": round(100.0 * passed / len(results), 1),
            "avg_tool_recall": round(sum(r.tool_recall for r in results) / len(results), 3),
            "stale_hallucination_rate_pct": round(100.0 * stale_hits / len(results), 1),
            "loop_abort_rate_pct": round(100.0 * loop_hits / len(results), 1),
            "self_heal_passed": bool(heal_res),
            "median_latency_ms": round(statistics.median(latencies), 1),
            "total_tokens": total_tokens,
        },
        "scenarios": [asdict(r) for r in results],
    }

    if args.save:
        RESULTS_DIR.mkdir(parents=True, exist_ok=True)
        safe_model = re.sub(r"[^A-Za-z0-9._-]+", "_", args.model)
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        out_file = RESULTS_DIR / f"{safe_model}_{stamp}.json"
        out_file.write_text(json.dumps(payload, indent=2), encoding="utf-8")
        write_markdown_report(RESULTS_DIR, REPORT_PATH)
        print(f"Saved results to {out_file} and updated {REPORT_PATH}")

    print(json.dumps(payload["summary"], indent=2))
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
