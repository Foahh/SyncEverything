#!/usr/bin/env python3
"""Track 5 Experiment 1 — Controlled Three-Baseline Continuation Comparison.

Compares three cross-device AI coding continuation architectures under identical
tasks, identical fixture repositories, identical LLM backend (deepseek-flash),
and deterministic ground-truth / human-oracle verification across N trials:

  1. `file_sync_only`  — Raw Git/file checkout + workspace tools + ask_human
  2. `static_template` — Raw Git/file checkout + frozen Commit-1 HANDOFF.md
  3. `relay_v1`        — Live `relay-daemon mcp-stdio` (DAG + STALE + worktree)

Four continuation tasks (`B1`–`B4`):
  - B1_decision_carryover : Non-default session decision (`alpha=0.25`) + constraints
  - B2_stale_metric_trap  : Commit 2 invalidated Commit 1 `calibration-latest.txt` / HANDOFF
  - B3_missing_dep_heal   : Undeclared vendored wheel + offline host + clean main tree
  - B4_defect_dirty_tree  : Runtime NameError + uncommitted user WIP in main README.md

Outputs pure measured timings, token usage, manual clarifications/corrections,
UI approval counts, first-pass vs final completion, and human-wait sensitivity
analysis (`T_eff(t_h) = T_wall + N_suppl * t_h + N_approve * t_a`) to:
  - course/bench/results/baseline_comparison_runs.jsonl
  - course/bench/results/baseline_comparison_summary.json
  - course/bench/results/baseline_comparison_summary.csv

Zero third-party Python dependencies (pure Python 3.11+ stdlib).
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_OUT = Path(__file__).resolve().parent
BENCH_DIR = DEFAULT_OUT.parent
COURSE_ROOT = BENCH_DIR.parent
REPO_ROOT = COURSE_ROOT.parent
sys.path.insert(0, str(COURSE_ROOT / "eval"))
from stale_metrics import forecast_error_values, reported_decimal_values

FIXTURE_SRC = BENCH_DIR / "fixture-repo"
BASELINES = ("file_sync_only", "static_template", "relay_v1")
TASKS = (
    "B1_decision_carryover",
    "B2_stale_metric_trap",
    "B3_missing_dep_heal",
    "B4_defect_dirty_tree",
)
TEXT_SUFFIXES = {".py", ".toml", ".lock", ".md", ".txt", ".sh", ".json"}
IGNORE_PATTERNS = shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc", ".DS_Store")
BLOCKED_PATH_PARTS = {".git", ".relay", ".relay-desktop", ".venv", "__pycache__"}
BLOCKED_SUFFIXES = (".db", ".db-wal", ".db-shm", ".pem", ".key", ".pyc")
WIP_MARKER_LINE = "\n<!-- USER_WIP_NOTE: validate window=28 seasonal spike next -->\n"


def load_dotenv(path: Path) -> None:
    if not path.is_file():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[7:].strip()
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if len(value) >= 2 and ((value[0] == '"' and value[-1] == '"') or (value[0] == "'" and value[-1] == "'")):
            value = value[1:-1]
        if key and not os.environ.get(key):
            os.environ[key] = value


def is_loopback_url(url: str) -> bool:
    host = (urllib.parse.urlparse(url).hostname or "").lower()
    return host in {"127.0.0.1", "localhost", "::1"}


def fixture_env(*, rust_log: str = "warn") -> dict[str, str]:
    env = os.environ.copy()
    for key in (
        "UV_DEFAULT_INDEX",
        "UV_INDEX",
        "UV_INDEX_URL",
        "UV_EXTRA_INDEX_URL",
        "PIP_INDEX_URL",
        "PIP_EXTRA_INDEX_URL",
    ):
        env.pop(key, None)
    env.update(
        {
            "UV_OFFLINE": "1",
            "UV_NO_CONFIG": "1",
            "UV_PYTHON_DOWNLOADS": "never",
            "UV_LINK_MODE": "copy",
            "GIT_TERMINAL_PROMPT": "0",
            "PYTHONIOENCODING": "utf-8",
            "RUST_LOG": rust_log,
        }
    )
    return env


def sanitize_text(text: str) -> str:
    cleaned = re.sub(r"[A-Za-z]:\\[^\s\"'`]+", "<LOCAL_PATH>", text)
    cleaned = re.sub(r"/Users/[^\s\"'`]+", "<LOCAL_PATH>", cleaned)
    cleaned = re.sub(r"sk-[A-Za-z0-9_-]{8,}", "<REDACTED_KEY>", cleaned)
    return cleaned


def find_daemon_binary(explicit: str | None) -> Path:
    if explicit:
        path = Path(explicit).resolve()
        if not path.is_file():
            raise FileNotFoundError(f"relay-daemon binary not found: {path}")
        return path
    exe = "relay-daemon.exe" if os.name == "nt" else "relay-daemon"
    for profile in ("debug", "release"):
        candidate = REPO_ROOT / "target" / profile / exe
        if candidate.is_file():
            return candidate
    raise FileNotFoundError("relay-daemon binary not found in target/debug or target/release.")


def run_cmd(args: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        args,
        cwd=str(cwd) if cwd else None,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
        env=fixture_env(),
    )
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"Command failed ({proc.returncode}): {' '.join(args)}\n"
            f"STDOUT:\n{proc.stdout}\nSTDERR:\n{proc.stderr}"
        )
    return proc


def init_fixture_repo(dest: Path) -> str:
    shutil.copytree(FIXTURE_SRC, dest, ignore=IGNORE_PATTERNS)
    for file_path in dest.rglob("*"):
        if file_path.is_file() and file_path.suffix.lower() in TEXT_SUFFIXES:
            text = file_path.read_text(encoding="utf-8")
            file_path.write_text(text, encoding="utf-8", newline="\n")
    (dest / ".gitignore").write_text(".venv/\n.relay/\n__pycache__/\n", encoding="utf-8", newline="\n")
    run_cmd(["git", "init", "--initial-branch=main"], cwd=dest)
    run_cmd(["git", "config", "core.autocrlf", "false"], cwd=dest)
    run_cmd(["git", "config", "user.name", "Relay Eval"], cwd=dest)
    run_cmd(["git", "config", "user.email", "eval@relay.local"], cwd=dest)
    run_cmd(["git", "add", "--all"], cwd=dest)
    run_cmd(["git", "commit", "-m", "chore(bench): initial forecast baseline"], cwd=dest)
    return run_cmd(["git", "rev-parse", "HEAD"], cwd=dest).stdout.strip()


def resolve_safe_path(root: Path, rel_path: str) -> Path:
    trimmed = rel_path.strip()
    if not trimmed:
        raise ValueError("path must not be empty")
    rel = Path(trimmed)
    if rel.is_absolute():
        raise ValueError("path must be repository-relative")
    for part in rel.parts:
        lower = part.lower()
        if part == ".." or lower in BLOCKED_PATH_PARTS or lower.startswith(".env") or lower.endswith(BLOCKED_SUFFIXES):
            raise ValueError(f"access to protected path '{trimmed}' is blocked")
    base = root.resolve()
    target = (root / rel).resolve()
    if target != base and base not in target.parents:
        raise ValueError(f"path '{trimmed}' escapes repository root")
    return target


def list_repo_files(repo: Path) -> str:
    files: list[str] = []
    for root_str, dirnames, filenames in os.walk(repo):
        dirnames[:] = sorted(d for d in dirnames if d not in BLOCKED_PATH_PARTS and not d.startswith(".env"))
        root_path = Path(root_str)
        for fname in sorted(filenames):
            lower = fname.lower()
            if lower.startswith(".env") or lower.endswith(BLOCKED_SUFFIXES):
                continue
            files.append((root_path / fname).relative_to(repo).as_posix())
    return json.dumps(files)


def read_repo_file(repo: Path, rel_path: str, start_line: int = 1, end_line: int = 220) -> str:
    target = resolve_safe_path(repo, rel_path)
    if not target.is_file():
        raise FileNotFoundError(f"file not found: {rel_path}")
    lines = target.read_text(encoding="utf-8").splitlines()
    start = max(1, start_line)
    end = min(len(lines), max(start, end_line))
    return "\n".join(f"{idx:>4}: {lines[idx - 1]}" for idx in range(start, end + 1))


class McpStdioClient:
    def __init__(self, daemon_bin: Path, repo_root: Path, data_dir: Path) -> None:
        self.proc = subprocess.Popen(
            [str(daemon_bin), "--repo-root", str(repo_root), "--data-dir", str(data_dir), "mcp-stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            env=fixture_env(),
        )
        self._next_id = 1
        self.tools: list[dict[str, Any]] = []
        self._stderr_lines: list[str] = []
        threading.Thread(target=self._drain_stderr, daemon=True).start()

    def _drain_stderr(self) -> None:
        if not self.proc.stderr:
            return
        for line in self.proc.stderr:
            self._stderr_lines.append(line)
            if len(self._stderr_lines) > 100:
                self._stderr_lines.pop(0)

    def __enter__(self) -> McpStdioClient:
        self.request(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "relay-track5-baseline", "version": "1.0.0"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self.tools = self.request("tools/list", {}).get("tools", [])
        return self

    def __exit__(self, *_: Any) -> None:
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

    def _send(self, msg: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        req_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._send(payload)
        assert self.proc.stdout is not None
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("mcp-stdio exited unexpectedly:\n" + "".join(self._stderr_lines))
            stripped = line.strip()
            if not stripped:
                continue
            resp = json.loads(stripped)
            if resp.get("id") == req_id:
                if "error" in resp:
                    raise RuntimeError(f"MCP error {method}: {resp['error']}")
                return resp["result"]

    def openai_tools(self) -> list[dict[str, Any]]:
        return [
            {
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t.get("description", ""),
                    "parameters": t.get("inputSchema") or {"type": "object", "properties": {}},
                },
            }
            for t in self.tools
        ]

    def call_tool(self, name: str, arguments: dict[str, Any]) -> tuple[bool, Any, str]:
        res = self.request("tools/call", {"name": name, "arguments": arguments})
        is_err = bool(res.get("isError", False))
        blocks = res.get("content", [])
        text = "\n".join(b.get("text", "") for b in blocks if b.get("type") == "text")
        parsed = None
        try:
            parsed = json.loads(text)
        except json.JSONDecodeError:
            pass
        return is_err, parsed, text


def insert_decision_node(db_path: Path, node_id: str, commit_sha: str, label: str, rationale: str, constraints: list[str]) -> None:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f000Z")
    conn = sqlite3.connect(str(db_path))
    try:
        conn.execute(
            """
            INSERT INTO dag_nodes (id, repo_id, label, payload_json, freshness_json, device_origin, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                node_id,
                "forecast-bench",
                label,
                json.dumps({"kind": "decision", "rationale": rationale, "constraints": constraints}),
                json.dumps({"status": "fresh"}),
                "laptop-edge",
                now,
                now,
            ),
        )
        conn.execute(
            """
            INSERT INTO dag_edges (id, repo_id, from_node, to_node, kind)
            VALUES (?, ?, ?, ?, ?)
            """,
            (f"informs:{node_id}", "forecast-bench", node_id, f"artifact:{commit_sha}", json.dumps("informs")),
        )
        conn.commit()
    finally:
        conn.close()


def insert_stale_decision_and_evidence(daemon_bin: Path, repo: Path, data: Path, note: str) -> str:
    submit_out = run_cmd(
        [
            str(daemon_bin),
            "--repo-root",
            str(repo),
            "--data-dir",
            str(data),
            "submit-task",
            "--preset",
            "train_grid",
            "--no-approval",
            "--note",
            note,
        ]
    ).stdout
    task_id = json.loads(submit_out)["spec"]["task_id"]
    with McpStdioClient(daemon_bin, repo, data) as mcp:
        err1, _, out1 = mcp.call_tool("run_preflight_in_worktree", {"task_id": task_id})
        if err1:
            raise RuntimeError(f"Setup preflight failed: {out1}")
        err2, _, out2 = mcp.call_tool("approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True})
        if err2:
            raise RuntimeError(f"Setup execution failed: {out2}")
    return task_id


def compute_expected_train_grid(repo: Path, alpha: float | None = None) -> dict[str, Any]:
    cmd = [sys.executable, "src/forecast.py", "--windows", "7,14,28"]
    if alpha is not None:
        cmd.extend(["--alpha", f"{alpha}"])
    proc = run_cmd(cmd, cwd=repo)
    return json.loads(proc.stdout)


def setup_task_fixture(
    task_id: str,
    daemon_bin: Path,
    case_dir: Path,
    *,
    use_relay: bool,
) -> dict[str, Any]:
    repo = case_dir / "repo"
    data = case_dir / "data"
    worktree = case_dir / "agent-worktree"
    commit1 = init_fixture_repo(repo)
    relay_task_id: str | None = None
    handoff_md = ""
    wip_before_bytes: bytes | None = None
    stale_mae: float | None = None

    if task_id == "B1_decision_carryover":
        expected = compute_expected_train_grid(repo, alpha=0.25)
        expected_mae = float(expected["best_mae"])
        expected_window = int(expected["best_window"])
        handoff_md = (
            "# Session Handoff (Commit {sha})\n"
            "- Commit SHA: `{commit}`\n"
            "- Decision: Use non-default smoothing factor `alpha = 0.25` with `windows = 7,14,28`.\n"
            "- Constraints: Run strictly offline (`UV_OFFLINE=1`) and keep the main working tree clean.\n"
            "- Status: Ready to run `train_grid` with `alpha=0.25` on HEAD.\n"
        ).format(sha=commit1[:7], commit=commit1)
        if use_relay:
            run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"])
            insert_decision_node(
                data / "relay.db",
                "decision:b1-alpha",
                commit1,
                "Use alpha=0.25 for seasonal grid sweep",
                "Laptop review selected EWMA smoothing alpha=0.25 over default 0.35.",
                [
                    "Use alpha=0.25 and windows=7,14,28",
                    "Run strictly offline (UV_OFFLINE=1) without modifying the main working tree",
                ],
            )
            submit_out = run_cmd(
                [
                    str(daemon_bin),
                    "--repo-root",
                    str(repo),
                    "--data-dir",
                    str(data),
                    "submit-task",
                    "--preset",
                    "train_grid",
                    "--param",
                    "alpha=0.25",
                    "--note",
                    "Session decision: keep windows=7,14,28, use alpha=0.25, run offline (UV_OFFLINE=1), keep main tree clean.",
                ]
            ).stdout
            relay_task_id = json.loads(submit_out)["spec"]["task_id"]
        prompt = (
            "Continue the cross-device forecasting handoff on current HEAD. "
            "Execute `train_grid` using all active session decisions and constraints, "
            "keep tracked files in the main working tree clean, and report the verified `best_mae` and `best_window`."
            + (f"\nQueued Relay Task ID: {relay_task_id}" if relay_task_id else "")
        )

    elif task_id == "B2_stale_metric_trap":
        commit1_metrics = compute_expected_train_grid(repo, alpha=0.35)
        stale_mae = float(commit1_metrics["best_mae"])
        handoff_md = (
            "# Session Handoff (Snapshot at Commit {sha})\n"
            "- Commit SHA: `{commit}`\n"
            "- Verified `train_grid` metrics at handoff time: `best_window = {win}`, `best_mae = {mae:.5f}`\n"
            "- Constraints: Windows 7,14,28; alpha=0.35; keep main working tree clean.\n"
        ).format(
            sha=commit1[:7],
            commit=commit1,
            win=commit1_metrics["best_window"],
            mae=stale_mae,
        )
        if use_relay:
            insert_stale_decision_and_evidence(daemon_bin, repo, data, "Commit 1 train_grid baseline")

        # Land Commit 2 modifying prediction formula while calibration-latest.txt & HANDOFF.md still hold Commit 1 MAE!
        forecast = repo / "src" / "forecast.py"
        text = forecast.read_text(encoding="utf-8").replace(
            "pred = 0.5 * ma + 0.5 * ewma", "pred = 0.4 * ma + 0.6 * ewma"
        )
        forecast.write_text(text, encoding="utf-8", newline="\n")
        run_cmd(["git", "add", "src/forecast.py"], cwd=repo)
        run_cmd(["git", "commit", "-m", "feat(forecast): shift blend to 0.4 MA / 0.6 EWMA"], cwd=repo)
        commit2 = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()
        expected = compute_expected_train_grid(repo, alpha=0.35)
        expected_mae = float(expected["best_mae"])
        expected_window = int(expected["best_window"])

        if use_relay:
            sync_out = json.loads(
                run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"]).stdout
            )
            assert sync_out.get("status") == "superseded", f"B2 expected superseded, got {sync_out}"
            submit_out = run_cmd(
                [
                    str(daemon_bin),
                    "--repo-root",
                    str(repo),
                    "--data-dir",
                    str(data),
                    "submit-task",
                    "--preset",
                    "train_grid",
                    "--note",
                    "Verify train_grid metrics for new HEAD commit after 0.4/0.6 blend update.",
                ]
            ).stdout
            relay_task_id = json.loads(submit_out)["spec"]["task_id"]

        prompt = (
            f"The previous developer handed off this repository on the Host at {commit1[:7]}, "
            f"and commit {commit2[:7]} is now at HEAD. "
            "Continue from the handoff state: report ONLY the verified fresh `best_mae` and `best_window` "
            "that apply to the current HEAD commit (do not cite outdated metrics from earlier commits as valid; "
            "run verification in an isolated worktree if needed while keeping the main working tree clean)."
            + (f"\nQueued Relay Task ID: {relay_task_id}" if relay_task_id else "")
        )

    elif task_id == "B3_missing_dep_heal":
        expected = compute_expected_train_grid(repo, alpha=0.35)
        expected_mae = float(expected["best_mae"])
        expected_window = int(expected["best_window"])
        handoff_md = (
            "# Session Handoff (Snapshot at Commit {sha})\n"
            "- Commit SHA: `{commit}`\n"
            "- Note: Initial pure-stdlib forecast baseline ran cleanly.\n"
            "- Constraints: Keep the Host main working tree clean; run strictly offline (`UV_OFFLINE=1`).\n"
        ).format(sha=commit1[:7], commit=commit1)
        forecast = repo / "src" / "forecast.py"
        forecast.write_text(
            forecast.read_text(encoding="utf-8") + "\nimport tiny_stats  # noqa: F401\n",
            encoding="utf-8",
            newline="\n",
        )
        run_cmd(["git", "add", "src/forecast.py"], cwd=repo)
        run_cmd(["git", "commit", "-m", "feat(forecast): use tiny_stats helper without updating lockfile"], cwd=repo)
        head = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()

        if use_relay:
            run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"])
            submit_out = run_cmd(
                [
                    str(daemon_bin),
                    "--repo-root",
                    str(repo),
                    "--data-dir",
                    str(data),
                    "submit-task",
                    "--preset",
                    "train_grid",
                    "--note",
                    "Run train_grid offline after tiny_stats import; keep main working tree clean.",
                ]
            ).stdout
            relay_task_id = json.loads(submit_out)["spec"]["task_id"]

        prompt = (
            f"Run `train_grid` on current HEAD ({head[:7]}). "
            "If preflight or execution fails due to a missing dependency, diagnose and fix it strictly offline "
            "without dirtying tracked files in the main working tree (use an isolated worktree), "
            "then report the verified `best_mae` and `best_window`."
            + (f"\nQueued Relay Task ID: {relay_task_id}" if relay_task_id else "")
        )

    elif task_id == "B4_defect_dirty_tree":
        handoff_md = (
            "# Session Handoff (Snapshot at Commit {sha})\n"
            "- Commit SHA: `{commit}`\n"
            "- Known issue: `--alpha` in `src/forecast.py` references `DEFAULT_ALPHA` which must equal `0.35`.\n"
            "- Constraints: Keep tracked source files in main working tree unmodified and preserve user WIP notes in README.md.\n"
        ).format(sha=commit1[:7], commit=commit1)
        forecast = repo / "src" / "forecast.py"
        broken = forecast.read_text(encoding="utf-8").replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=DEFAULT_ALPHA, help="EWMA smoothing factor")',
        )
        forecast.write_text(broken, encoding="utf-8", newline="\n")
        run_cmd(["git", "add", "src/forecast.py"], cwd=repo)
        run_cmd(["git", "commit", "-m", "feat(forecast): reference DEFAULT_ALPHA constant"], cwd=repo)
        head = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()

        # Simulate developer's uncommitted WIP note in main README.md BEFORE continuation
        readme = repo / "README.md"
        readme.write_text(readme.read_text(encoding="utf-8") + WIP_MARKER_LINE, encoding="utf-8", newline="\n")
        wip_before_bytes = readme.read_bytes()

        expected = {"best_mae": 2.16502, "best_window": 28}
        expected_mae = 2.16502
        expected_window = 28

        if use_relay:
            run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"])
            submit_out = run_cmd(
                [
                    str(daemon_bin),
                    "--repo-root",
                    str(repo),
                    "--data-dir",
                    str(data),
                    "submit-task",
                    "--preset",
                    "train_grid",
                    "--note",
                    "Fix DEFAULT_ALPHA=0.35 in isolated worktree without touching dirty README WIP in main checkout.",
                ]
            ).stdout
            relay_task_id = json.loads(submit_out)["spec"]["task_id"]

        prompt = (
            f"Run `train_grid` on current HEAD ({head[:7]}). "
            "There is a code defect in `src/forecast.py` and the user has uncommitted WIP edits in the main checkout's `README.md`. "
            "Diagnose and fix the defect (`DEFAULT_ALPHA = 0.35`) in an isolated worktree without modifying any file "
            "in the main working tree or disturbing `README.md`, then complete `train_grid` and report the verified `best_mae` and `best_window`."
            + (f"\nQueued Relay Task ID: {relay_task_id}" if relay_task_id else "")
        )
    else:
        raise ValueError(f"Unknown task_id: {task_id}")

    return {
        "task_id": task_id,
        "repo": repo,
        "data": data,
        "worktree": worktree,
        "relay_task_id": relay_task_id,
        "handoff_md": handoff_md,
        "expected_mae": expected_mae,
        "expected_window": expected_window,
        "stale_mae": stale_mae,
        "wip_before_bytes": wip_before_bytes,
        "prompt": prompt,
    }


def workspace_tools_openai(include_handoff_template: bool) -> list[dict[str, Any]]:
    tools = [
        {
            "type": "function",
            "function": {
                "name": "list_repo_files",
                "description": "List files inside the repository checkout.",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
            },
        },
        {
            "type": "function",
            "function": {
                "name": "read_repo_file",
                "description": "Read a UTF-8 file from main checkout or isolated worktree with 1-based line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "start_line": {"type": "integer", "minimum": 1},
                        "end_line": {"type": "integer", "minimum": 1},
                        "in_worktree": {"type": "boolean"},
                    },
                    "required": ["path"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "write_repo_file",
                "description": "Write full content to a relative file inside main or isolated worktree (`in_worktree=true` keeps main clean).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"},
                        "in_worktree": {"type": "boolean"},
                    },
                    "required": ["path", "content"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "run_repo_cmd",
                "description": (
                    "Execute a whitelisted safe command (`git_status`, `git_log`, `git_diff`, `create_worktree`, "
                    "`restore_main_files`, `smoke`, `train_grid`, `uv_add_vendor`). Set target='worktree' after `create_worktree`."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "cmd_kind": {
                            "type": "string",
                            "enum": [
                                "git_status",
                                "git_log",
                                "git_diff",
                                "create_worktree",
                                "restore_main_files",
                                "smoke",
                                "train_grid",
                                "uv_add_vendor",
                            ],
                        },
                        "target": {"type": "string", "enum": ["main", "worktree"]},
                        "alpha": {"type": "number"},
                        "windows": {"type": "string"},
                    },
                    "required": ["cmd_kind"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "ask_human_clarification",
                "description": "Ask the human developer for missing session decisions, constraints, dependency instructions, or verification status.",
                "parameters": {
                    "type": "object",
                    "properties": {"question": {"type": "string"}},
                    "required": ["question"],
                },
            },
        },
    ]
    if include_handoff_template:
        tools.append(
            {
                "type": "function",
                "function": {
                    "name": "read_handoff_template",
                    "description": "Read the static HANDOFF.md markdown template captured when the handoff was pushed.",
                    "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
                },
            }
        )
    return tools


def relay_local_helpers_openai() -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": "list_repo_files",
                "description": "Read-only helper: list non-sensitive files inside the repository checkout.",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
            },
        },
        {
            "type": "function",
            "function": {
                "name": "read_repo_file",
                "description": "Read-only helper: read UTF-8 file lines from main checkout with 1-based line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "start_line": {"type": "integer", "minimum": 1},
                        "end_line": {"type": "integer", "minimum": 1},
                    },
                    "required": ["path"],
                },
            },
        },
    ]


def oracle_clarification_answer(setup: dict[str, Any], question: str) -> str:
    tid = setup["task_id"]
    if tid == "B1_decision_carryover":
        return (
            "Session decisions: evaluate `train_grid` with `windows = 7,14,28` and agreed smoothing parameter "
            "`alpha = 0.25`. Run strictly offline (`UV_OFFLINE=1`) and keep tracked files in main clean."
        )
    if tid == "B2_stale_metric_trap":
        return (
            "Commit 2 modified the blending formula in `src/forecast.py` AFTER `results/calibration-latest.txt` "
            "was generated, so earlier MAE numbers (2.16502) are STALE. Run `train_grid` on current HEAD in an isolated worktree."
        )
    if tid == "B3_missing_dep_heal":
        return (
            "Host is offline (`UV_OFFLINE=1`). Do not edit the main checkout: call `create_worktree`, run "
            "`uv_add_vendor` in `target='worktree'` (using `./vendor/tiny_stats-0.1.0-py3-none-any.whl`), then run `train_grid` in `worktree`."
        )
    if tid == "B4_defect_dirty_tree":
        return (
            "Call `create_worktree`, define `DEFAULT_ALPHA = 0.35` in `worktree`'s `src/forecast.py` via "
            "`write_repo_file(in_worktree=true)`, keep `main` `README.md` untouched, and run `train_grid` in `worktree`."
        )
    return "Proceed according to repository constraints and keep main working tree clean."


def execute_workspace_tool(
    name: str,
    args: dict[str, Any],
    setup: dict[str, Any],
    state: dict[str, Any],
) -> tuple[bool, str]:
    repo: Path = setup["repo"]
    worktree: Path = setup["worktree"]

    def _root(in_wt: bool) -> Path:
        if in_wt:
            if not worktree.exists():
                raise RuntimeError("Isolated worktree has not been created yet; call run_repo_cmd(cmd_kind='create_worktree') first.")
            return worktree
        return repo

    try:
        if name == "list_repo_files":
            return False, list_repo_files(repo)
        if name == "read_repo_file":
            root = _root(bool(args.get("in_worktree", False)))
            return False, read_repo_file(root, str(args.get("path", "")), int(args.get("start_line", 1)), int(args.get("end_line", 220)))
        if name == "write_repo_file":
            root = _root(bool(args.get("in_worktree", False)))
            rel = str(args.get("path", ""))
            target = resolve_safe_path(root, rel)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(str(args.get("content", "")), encoding="utf-8", newline="\n")
            return False, json.dumps({"written": rel, "in_worktree": bool(args.get("in_worktree", False))})
        if name == "read_handoff_template":
            return False, setup["handoff_md"]
        if name == "ask_human_clarification":
            state["clarifications"] += 1
            return False, oracle_clarification_answer(setup, str(args.get("question", "")))
        if name == "run_repo_cmd":
            kind = str(args.get("cmd_kind", ""))
            target_kind = str(args.get("target", "main"))
            cwd = worktree if (target_kind == "worktree" and kind != "create_worktree") else repo
            if target_kind == "worktree" and kind != "create_worktree" and not worktree.exists():
                return True, "Worktree does not exist yet; call create_worktree first."
            if kind == "git_status":
                proc = run_cmd(["git", "status", "--short"], cwd=cwd, check=False)
                return proc.returncode != 0, proc.stdout or "(clean)"
            if kind == "git_log":
                proc = run_cmd(["git", "log", "-n", "5", "--stat", "--oneline"], cwd=cwd, check=False)
                return proc.returncode != 0, proc.stdout
            if kind == "git_diff":
                proc = run_cmd(["git", "diff"], cwd=cwd, check=False)
                return proc.returncode != 0, proc.stdout or "(no diff)"
            if kind == "create_worktree":
                if not worktree.exists():
                    run_cmd(["git", "worktree", "add", "--detach", str(worktree), "HEAD"], cwd=repo)
                return False, json.dumps({"worktree_ready": True, "branch": "detached-HEAD"})
            if kind == "restore_main_files":
                run_cmd(["git", "checkout", "HEAD", "--", "src/forecast.py", "pyproject.toml", "uv.lock"], cwd=repo)
                return False, "Restored src/forecast.py, pyproject.toml, and uv.lock on main without touching README.md."
            if kind == "uv_add_vendor":
                proc = run_cmd(
                    ["uv", "add", "--offline", "--", "./vendor/tiny_stats-0.1.0-py3-none-any.whl"],
                    cwd=cwd,
                    check=False,
                )
                return proc.returncode != 0, sanitize_text(proc.stdout + proc.stderr) or "Added vendor wheel"
            if kind == "smoke":
                proc = run_cmd(["uv", "run", "python", "src/forecast.py", "--smoke"], cwd=cwd, check=False)
                return proc.returncode != 0, sanitize_text(proc.stdout + proc.stderr)
            if kind == "train_grid":
                windows_str = str(args.get("windows") or "7,14,28")
                if not re.fullmatch(r"\d+(,\d+)*", windows_str):
                    return True, f"Invalid windows parameter: {windows_str}"
                cmd = ["uv", "run", "python", "src/forecast.py", "--windows", windows_str]
                alpha_val = args.get("alpha")
                if alpha_val is not None:
                    cmd.extend(["--alpha", f"{float(alpha_val)}"])
                proc = run_cmd(cmd, cwd=cwd, check=False)
                if proc.returncode == 0:
                    try:
                        parsed = json.loads(proc.stdout)
                        state["executed_train_grids"].append(parsed)
                    except (KeyError, ValueError, json.JSONDecodeError):
                        pass
                return proc.returncode != 0, sanitize_text(proc.stdout + proc.stderr)
            return True, f"Unknown cmd_kind: {kind}"
        return True, f"Unknown tool: {name}"
    except Exception as exc:  # noqa: BLE001
        return True, sanitize_text(f"error: {exc}")


def main_tree_preserved(setup: dict[str, Any]) -> tuple[bool, list[str]]:
    repo: Path = setup["repo"]
    lines = [line for line in run_cmd(["git", "status", "--porcelain"], cwd=repo).stdout.splitlines() if line.strip()]
    issues: list[str] = []
    if setup["task_id"] == "B4_defect_dirty_tree":
        actual_bytes = (repo / "README.md").read_bytes()
        if actual_bytes != setup["wip_before_bytes"]:
            issues.append("main README.md uncommitted WIP edit was modified or lost")
        extra = [ln for ln in lines if not ln.endswith(" README.md")]
        if extra:
            issues.append(f"main checkout has unexpected dirty/untracked files: {extra}")
    else:
        if lines:
            issues.append(f"main checkout working tree is dirty: {lines}")
    return len(issues) == 0, issues


def matches_grid(metrics: Any, alpha: float, mae: float, window: int) -> bool:
    if not isinstance(metrics, dict) or not isinstance(metrics.get("runs"), list):
        return False
    windows = [row.get("window") for row in metrics["runs"] if isinstance(row, dict)]
    return (
        metrics.get("mode") == "grid" and metrics.get("alpha") == alpha
        and metrics.get("best_mae") == mae and metrics.get("best_window") == window
        and len(metrics["runs"]) == 3 and all(w in windows for w in (7, 14, 28))
    )


def check_ground_truth(
    baseline: str,
    setup: dict[str, Any],
    state: dict[str, Any],
    answer: str,
    mcp: McpStdioClient | None,
) -> tuple[bool, list[str], bool]:
    """Return (passed, failures, stale_citation_detected)."""
    failures: list[str] = []
    stale_cited = False
    expected_mae = setup["expected_mae"]
    expected_win = setup["expected_window"]
    mae_tok = f"{expected_mae:.5f}"

    # Check clean main working tree / WIP preservation
    tree_ok, tree_issues = main_tree_preserved(setup)
    if not tree_ok:
        failures.extend(tree_issues)

    # Check actual fresh train_grid execution on HEAD
    if baseline == "relay_v1":
        assert mcp is not None
        _, snap, _ = mcp.call_tool("get_dag_snapshot", {})
        fresh_ev = [
            n
            for n in (snap or {}).get("nodes", [])
            if (n.get("payload") or {}).get("kind") == "evidence"
            and n["payload"].get("task_id") == setup["relay_task_id"]
            and n["payload"].get("preset") == "train_grid"
            and (n.get("freshness") or {}).get("status") == "fresh"
        ]
        runs = [node["payload"]["metrics_json"] for node in fresh_ev]
    else:
        runs = state["executed_train_grids"]
    expected_alpha = 0.25 if setup["task_id"] == "B1_decision_carryover" else 0.35
    if not any(matches_grid(r, expected_alpha, expected_mae, expected_win) for r in runs):
        failures.append(f"Missing verified HEAD grid for windows=7,14,28, alpha={expected_alpha}, "
                        f"best_mae={mae_tok}, best_window={expected_win}.")

    reported = reported_decimal_values(answer)
    mentions_head_mae = any(abs(value - expected_mae) <= 1e-4 for value in reported)
    if not mentions_head_mae:
        failures.append(f"Final answer must report verified HEAD best_mae {mae_tok}.")
    if not re.search(rf"(?<![\w.]){expected_win}(?:\.0+)?(?!\w|\.\d)", answer):
        failures.append(f"Final answer must report verified best_window {expected_win}.")

    if setup["task_id"] == "B2_stale_metric_trap" and setup["stale_mae"] is not None:
        if reported & forecast_error_values({"best_mae": setup["stale_mae"]}):
            lower = answer.lower()
            explicitly_rejected = any(w in lower for w in ("stale", "outdated", "invalid", "superseded", "old", "expired"))
            stale_cited = not explicitly_rejected or not mentions_head_mae
            if stale_cited:
                failures.append("Cited stale Commit-1 metric (2.16502) instead of verified HEAD best_mae.")

    return len(failures) == 0, failures, stale_cited


def build_oracle_correction(setup: dict[str, Any], failures: list[str], baseline: str) -> str:
    tid = setup["task_id"]
    joined = " ".join(failures)
    if baseline == "relay_v1":
        return (
            f"Verification check failed: {'; '.join(failures)}. "
            "Complete the Relay MCP workflow (`run_preflight_in_worktree`, `propose_worktree_patch` if smoke fails, "
            f"`approve_and_execute_task`) for task {setup['relay_task_id']} and report the verified fresh metrics."
        )
    hints = [f"Human reviewer feedback — remaining issues: {'; '.join(failures)}."]
    if "dirty" in joined or "README.md" in joined:
        hints.append("Run `run_repo_cmd(cmd_kind='restore_main_files')` to keep main clean, then use an isolated worktree (`create_worktree`).")
    if tid == "B1_decision_carryover":
        hints.append("Use the session's agreed smoothing factor `alpha = 0.25` and `windows = 7,14,28` when running `train_grid`.")
    elif tid == "B2_stale_metric_trap":
        hints.append("Commit 2 invalidated `results/calibration-latest.txt` and `HANDOFF.md` (2.16502 is stale). Run `train_grid` in an isolated worktree on HEAD and report ONLY the new fresh `best_mae` and `best_window`.")
    elif tid == "B3_missing_dep_heal":
        hints.append("Host is offline (`UV_OFFLINE=1`). Create an isolated worktree (`create_worktree`), call `run_repo_cmd(cmd_kind='uv_add_vendor', target='worktree')`, then run `train_grid` in `worktree`.")
    elif tid == "B4_defect_dirty_tree":
        hints.append("Create an isolated worktree (`create_worktree`), fix `DEFAULT_ALPHA = 0.35` in `worktree`'s `src/forecast.py` (`in_worktree=true`), keep `main` untouched, and run `train_grid` in `worktree`.")
    return " ".join(hints)


def is_retryable_ollama_xml_500(code: int, detail: str) -> bool:
    lower = detail.lower()
    return code == 500 and ("xml syntax error" in lower or "unexpected eof" in lower)


def call_openai_chat(
    base_url: str,
    model: str,
    api_key: str | None,
    messages: list[dict[str, Any]],
    tools: list[dict[str, Any]],
    timeout_secs: int,
    max_tokens: int | None = None,
) -> tuple[dict[str, Any], float]:
    normalized = base_url.strip().rstrip("/")
    url = normalized if normalized.endswith("/chat/completions") else f"{normalized}/chat/completions"
    body: dict[str, Any] = {
        "model": model,
        "messages": messages,
        "temperature": 0.0,
        "stream": False,
    }
    if tools:
        body["tools"] = tools
        body["tool_choice"] = "auto"
    if max_tokens is not None:
        body["max_tokens"] = max_tokens
    headers = {"Content-Type": "application/json", "User-Agent": "Relay-Baseline-Eval/1.0"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    payload = json.dumps(body).encode("utf-8")
    t0 = time.perf_counter()
    for attempt in range(2):
        req = urllib.request.Request(url, data=payload, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
                raw = resp.read().decode("utf-8")
            return json.loads(raw), (time.perf_counter() - t0) * 1000.0
        except urllib.error.HTTPError as err:
            with err:
                detail = err.read().decode("utf-8", errors="replace")
            if attempt == 0 and is_retryable_ollama_xml_500(err.code, detail):
                continue
            raise RuntimeError(f"HTTP {err.code}: {detail[:300]}") from err
    raise RuntimeError(f"Failed request to {url}")


def run_dry_baseline_case(baseline: str, setup: dict[str, Any], daemon_bin: Path) -> dict[str, Any]:
    t_start = time.perf_counter()
    state = {"clarifications": 0, "corrections": 0, "ui_approvals": 0, "executed_train_grids": []}
    tid = setup["task_id"]
    answer = ""
    if baseline == "relay_v1":
        with McpStdioClient(daemon_bin, setup["repo"], setup["data"]) as mcp:
            task_arg = {"task_id": setup["relay_task_id"]}
            if tid == "B1_decision_carryover":
                mcp.call_tool("get_dag_snapshot", {})
                mcp.call_tool("list_handoffs", {"limit": 5})
            elif tid == "B2_stale_metric_trap":
                mcp.call_tool("get_dag_snapshot", {})
            elif tid == "B3_missing_dep_heal":
                mcp.call_tool("run_preflight_in_worktree", task_arg)
                mcp.call_tool(
                    "propose_worktree_patch",
                    {
                        "task_id": setup["relay_task_id"],
                        "patch_kind": "dependency_spec",
                        "summary": "Install vendored tiny_stats wheel offline",
                        "dependency_spec": "./vendor/tiny_stats-0.1.0-py3-none-any.whl",
                    },
                )
            elif tid == "B4_defect_dirty_tree":
                mcp.call_tool("run_preflight_in_worktree", task_arg)
                mcp.call_tool(
                    "read_worktree_file",
                    {"task_id": setup["relay_task_id"], "file_path": "src/forecast.py", "start_line": 1, "end_line": 40},
                )
                mcp.call_tool(
                    "propose_worktree_patch",
                    {
                        "task_id": setup["relay_task_id"],
                        "patch_kind": "code_diff",
                        "summary": "Define DEFAULT_ALPHA = 0.35",
                        "edits": [
                            {
                                "file_path": "src/forecast.py",
                                "old_text": "def generate_series(",
                                "new_text": "DEFAULT_ALPHA = 0.35\n\n\ndef generate_series(",
                            }
                        ],
                    },
                )
            if tid in {"B1_decision_carryover", "B2_stale_metric_trap"}:
                mcp.call_tool("run_preflight_in_worktree", task_arg)
            _, gate, _ = mcp.call_tool("approve_and_execute_task", task_arg)
            if isinstance(gate, dict) and gate.get("outcome") == "approval_required":
                state["ui_approvals"] += 1
            mcp.call_tool("approve_and_execute_task", {"task_id": setup["relay_task_id"], "approved_via_ui": True})
            _, snap, _ = mcp.call_tool("get_dag_snapshot", {})
            fresh = [
                n
                for n in snap["nodes"]
                if n["payload"]["kind"] == "evidence" and (n.get("freshness") or {}).get("status") == "fresh"
            ]
            m = fresh[-1]["payload"]["metrics_json"]
            answer = f"Verified HEAD best_mae={m['best_mae']:.5f}, best_window={m['best_window']}."
            passed, failures, stale_cited = check_ground_truth(baseline, setup, state, answer, mcp)
    else:
        if baseline == "static_template":
            execute_workspace_tool("read_handoff_template", {}, setup, state)
        if tid == "B1_decision_carryover":
            if baseline == "file_sync_only":
                execute_workspace_tool("ask_human_clarification", {"question": "What parameters were agreed?"}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "train_grid", "target": "main", "alpha": 0.25}, setup, state)
        elif tid == "B2_stale_metric_trap":
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "create_worktree"}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "train_grid", "target": "worktree"}, setup, state)
        elif tid == "B3_missing_dep_heal":
            if baseline == "file_sync_only":
                execute_workspace_tool("ask_human_clarification", {"question": "Where is tiny_stats?"}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "create_worktree"}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "uv_add_vendor", "target": "worktree"}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "train_grid", "target": "worktree"}, setup, state)
        elif tid == "B4_defect_dirty_tree":
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "create_worktree"}, setup, state)
            orig = (setup["worktree"] / "src" / "forecast.py").read_text(encoding="utf-8")
            patched = orig.replace("DEFAULT_ALPHA", "0.35", 1)
            execute_workspace_tool("write_repo_file", {"path": "src/forecast.py", "content": patched, "in_worktree": True}, setup, state)
            execute_workspace_tool("run_repo_cmd", {"cmd_kind": "train_grid", "target": "worktree"}, setup, state)
        last = state["executed_train_grids"][-1]
        answer = f"Verified HEAD best_mae={last['best_mae']:.5f}, best_window={last['best_window']}."
        passed, failures, stale_cited = check_ground_truth(baseline, setup, state, answer, None)

    if not passed:
        raise RuntimeError(f"Dry-run failed on {baseline}/{tid}: {failures}")
    wall_ms = (time.perf_counter() - t_start) * 1000.0
    return {
        "baseline": baseline,
        "task_id": tid,
        "final_passed": passed,
        "manual_supplements": state["clarifications"] + state["corrections"],
        "ui_approvals": state["ui_approvals"],
        "wall_clock_ms": round(wall_ms, 1),
    }


def system_prompt_for(baseline: str) -> str:
    if baseline == "relay_v1":
        return (
            "You are the Relay v1.0 execution agent connected to Relay's MCP server plus read-only file inspection helpers.\n"
            "Rules:\n"
            "1. Inspect `get_dag_snapshot` and `list_handoffs` first to capture session decisions, constraints, and STALE quarantine warnings. `get_dag_snapshot` is the sole authoritative source (SSOT) for verified experiment metrics.\n"
            "2. NEVER cite a `stale` evidence node's metric or historical static text files as valid for HEAD when `fresh_evidence_count == 0`.\n"
            "3. For queued tasks: call `run_preflight_in_worktree(task_id)`. If preflight fails: for missing Python module use `propose_worktree_patch(task_id=..., patch_kind='dependency_spec', summary=..., dependency_spec='./vendor/...')` pointing to any local `./vendor/*.whl` file found via `list_repo_files`; for a code bug, inspect `error_snippets` or call `read_worktree_file` and call `propose_worktree_patch(task_id=..., patch_kind='code_diff', summary=..., edits=[{\"file_path\": ..., \"old_text\": ..., \"new_text\": ...}])`.\n"
            "4. Call `approve_and_execute_task(task_id)` first; if `approval_required` is returned, pause or mention approval is needed and proceed when approved."
        )
    extra = (
        "You ALSO have `read_handoff_template` containing the static HANDOFF.md written at initial handoff time. "
        "Note that static templates can become outdated if newer git commits landed afterward — verify with git tools.\n"
        if baseline == "static_template"
        else ""
    )
    return (
        "You are a coding & experiment continuation agent working on a synced local repository.\n"
        + extra
        + "Rules:\n"
        "1. Preserve the main working tree: NEVER leave tracked files (`src/forecast.py`, `pyproject.toml`, `uv.lock`) modified on `main`, and NEVER overwrite uncommitted user notes in `README.md`. Use `run_repo_cmd(cmd_kind='create_worktree')` + `target='worktree'` / `in_worktree=true` whenever editing dependencies or source files.\n"
        "2. Only cite metrics verified on current HEAD (if newer commits landed after cached results, re-run `train_grid` on HEAD).\n"
        "3. Check files/git first before calling `ask_human_clarification`; only ask when required constraints or offline dependency details are genuinely missing from repo context.\n"
        "4. Always report the verified HEAD `best_mae` and `best_window` cleanly."
    )


def run_live_case(
    baseline: str,
    setup: dict[str, Any],
    daemon_bin: Path,
    llm_cfg: dict[str, Any],
    max_turns: int,
    max_corrections: int,
    timeout_secs: int,
) -> dict[str, Any]:
    wall_t0 = time.perf_counter()
    llm_ms = 0.0
    tool_ms = 0.0
    p_tok = 0
    c_tok = 0
    llm_turns = 0
    tool_calls = 0
    seen_calls: dict[str, int] = {}
    ui_approvals: set[tuple[str, str | None]] = set()
    state = {"clarifications": 0, "corrections": 0, "ui_approvals": 0, "executed_train_grids": []}
    stale_first_pass = False
    first_pass_recorded = False
    first_pass_passed = False
    final_passed = False
    loop_aborted = False
    final_answer = ""
    last_failures: list[str] = []

    mcp_ctx = McpStdioClient(daemon_bin, setup["repo"], setup["data"]) if baseline == "relay_v1" else None
    try:
        if mcp_ctx is not None:
            mcp_ctx.__enter__()
            tools = mcp_ctx.openai_tools() + relay_local_helpers_openai()
        else:
            tools = workspace_tools_openai(include_handoff_template=(baseline == "static_template"))

        messages: list[dict[str, Any]] = [
            {"role": "system", "content": system_prompt_for(baseline)},
            {"role": "user", "content": setup["prompt"]},
        ]

        turn_limit = max_turns
        while llm_turns < turn_limit:
            turn_idx = llm_turns
            llm_turns += 1
            try:
                resp, elapsed = call_openai_chat(
                    llm_cfg["base_url"],
                    llm_cfg["model"],
                    llm_cfg["api_key"],
                    messages,
                    tools,
                    timeout_secs=timeout_secs,
                )
            except Exception as exc:  # noqa: BLE001
                loop_aborted = True
                last_failures = [sanitize_text(f"LLM error: {exc}")]
                break
            llm_ms += elapsed
            usage = resp.get("usage") or {}
            p_tok += int(usage.get("prompt_tokens", 0))
            c_tok += int(usage.get("completion_tokens", 0))

            choice = resp["choices"][0]["message"]
            tool_calls_list = choice.get("tool_calls") or []
            norm_calls: list[dict[str, Any]] = []
            for idx, tc in enumerate(tool_calls_list):
                tc_copy = dict(tc)
                if not tc_copy.get("id"):
                    tc_copy["id"] = f"call_{turn_idx}_{idx}"
                norm_calls.append(tc_copy)
            assistant_msg: dict[str, Any] = dict(choice)
            assistant_msg["content"] = choice.get("content") or ""
            if norm_calls:
                assistant_msg["tool_calls"] = norm_calls
            messages.append(assistant_msg)

            if not norm_calls:
                final_answer = (choice.get("content") or "").strip()
                passed, failures, stale_cited = check_ground_truth(baseline, setup, state, final_answer, mcp_ctx)
                if not first_pass_recorded:
                    first_pass_recorded = True
                    stale_first_pass = stale_cited
                    first_pass_passed = passed and state["clarifications"] == 0
                if passed:
                    final_passed = True
                    last_failures = []
                    break
                last_failures = failures
                if state["corrections"] >= max_corrections:
                    break
                state["corrections"] += 1
                turn_limit = llm_turns + 6
                messages.append({"role": "user", "content": build_oracle_correction(setup, failures, baseline)})
                seen_calls.clear()
                continue

            pending_ui_approvals: set[tuple[str, str | None]] = set()
            for tc in norm_calls:
                fn = tc["function"]
                name = fn["name"]
                raw_args = fn.get("arguments") or "{}"
                try:
                    parsed = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
                    if not isinstance(parsed, dict):
                        parsed = {}
                except json.JSONDecodeError:
                    parsed = {}

                sig = f"{name}:{json.dumps(parsed, sort_keys=True)}"
                seen_calls[sig] = seen_calls.get(sig, 0) + 1
                if seen_calls[sig] >= 3:
                    loop_aborted = True
                    last_failures = [f"Repeated identical tool call aborted: {name}"]
                    break

                t_tool0 = time.perf_counter()
                tool_calls += 1
                if mcp_ctx is not None:
                    if name == "list_repo_files":
                        output_text = list_repo_files(setup["repo"])
                    elif name == "read_repo_file":
                        try:
                            output_text = read_repo_file(
                                setup["repo"],
                                str(parsed.get("path", "")),
                                int(parsed.get("start_line", 1)),
                                int(parsed.get("end_line", 220)),
                            )
                        except Exception as exc:  # noqa: BLE001
                            output_text = f"error: {exc}"
                    elif name == "approve_and_execute_task" and parsed.get("approved_via_ui") and (
                        not isinstance(parsed.get("task_id"), str)
                        or not isinstance(parsed.get("patch_id"), (str, type(None)))
                        or (parsed["task_id"], parsed.get("patch_id")) not in ui_approvals
                    ):
                        output_text = "No reviewer approval has been granted for this task and patch."
                    else:
                        _, payload, output_text = mcp_ctx.call_tool(name, parsed)
                        if (
                            name == "approve_and_execute_task"
                            and isinstance(payload, dict)
                            and payload.get("outcome") == "approval_required"
                        ):
                            pending_ui_approvals.add((payload["task_id"], payload.get("patch_id")))
                else:
                    _, output_text = execute_workspace_tool(name, parsed, setup, state)
                tool_ms += (time.perf_counter() - t_tool0) * 1000.0
                messages.append({"role": "tool", "tool_call_id": tc["id"], "content": sanitize_text(output_text)})

            if loop_aborted:
                break
            for gate in sorted(pending_ui_approvals - ui_approvals, key=str):
                ui_approvals.add(gate)
                state["ui_approvals"] += 1
                messages.append(
                    {
                        "role": "user",
                        "content": (
                            f"Human reviewer approved task {gate[0]}, patch {gate[1]} on the Relay card. "
                            "Call `approve_and_execute_task` now with `approved_via_ui=true` (and the same `task_id` / `patch_id`)."
                        ),
                    }
                )
        else:
            loop_aborted = True
            last_failures.append(f"Exceeded the model turn budget ({turn_limit} turns).")
    finally:
        if mcp_ctx is not None:
            mcp_ctx.__exit__(None, None, None)

    wall_ms = (time.perf_counter() - wall_t0) * 1000.0
    tree_ok, _ = main_tree_preserved(setup)
    manual_suppl = state["clarifications"] + state["corrections"]
    return {
        "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "baseline": baseline,
        "task_id": setup["task_id"],
        "model": llm_cfg["model"],
        "first_pass_passed": first_pass_passed,
        "final_passed": final_passed,
        "stale_hallucination_first_pass": stale_first_pass,
        "main_tree_preserved": tree_ok,
        "manual_supplements": manual_suppl,
        "clarifications": state["clarifications"],
        "corrections": state["corrections"],
        "ui_approvals": state["ui_approvals"],
        "wall_clock_ms": round(wall_ms, 1),
        "llm_latency_ms": round(llm_ms, 1),
        "tool_exec_ms": round(tool_ms, 1),
        "llm_turns": llm_turns,
        "tool_calls": tool_calls,
        "loop_aborted": loop_aborted,
        "prompt_tokens": p_tok,
        "completion_tokens": c_tok,
        "total_tokens": p_tok + c_tok,
        "failures": [sanitize_text(f) for f in last_failures],
        "answer_excerpt": sanitize_text(final_answer[:600]),
    }


def _mean_sd(values: list[float]) -> tuple[float, float]:
    return statistics.mean(values), (statistics.stdev(values) if len(values) >= 2 else 0.0)


def summarize_results(records: list[dict[str, Any]], daemon_bin: Path, model_name: str, trials: int) -> dict[str, Any]:
    baseline_summaries: list[dict[str, Any]] = []
    per_task_summaries: list[dict[str, Any]] = []
    sensitivity_rows: list[dict[str, Any]] = []
    wait_grid_secs = (0, 15, 30, 60)
    approval_wait_s = 5.0

    for b in BASELINES:
        subset = [r for r in records if r["baseline"] == b]
        n = len(subset)
        wall_s_list = [r["wall_clock_ms"] / 1000.0 for r in subset]
        suppl_list = [float(r["manual_supplements"]) for r in subset]
        ui_list = [float(r["ui_approvals"]) for r in subset]
        tok_list = [float(r["total_tokens"]) for r in subset]
        wall_m, wall_sd = _mean_sd(wall_s_list)
        suppl_m, suppl_sd = _mean_sd(suppl_list)
        ui_m, ui_sd = _mean_sd(ui_list)
        tok_m, _ = _mean_sd(tok_list)

        # Effective turnaround across trial-level observations for each t_h
        eff_sensitivity: dict[str, Any] = {}
        for t_h in wait_grid_secs:
            eff_vals = [
                (r["wall_clock_ms"] / 1000.0) + r["manual_supplements"] * float(t_h) + r["ui_approvals"] * approval_wait_s
                for r in subset
            ]
            em, esd = _mean_sd(eff_vals)
            eff_sensitivity[f"th_{t_h}s"] = {"mean_s": round(em, 2), "sd_s": round(esd, 2)}
            sensitivity_rows.append(
                {"baseline": b, "human_wait_s": t_h, "approval_wait_s": approval_wait_s, "eff_time_mean_s": round(em, 2), "eff_time_sd_s": round(esd, 2)}
            )

        baseline_summaries.append(
            {
                "baseline": b,
                "n_runs": n,
                "first_pass_success_pct": round(100.0 * sum(1 for r in subset if r["first_pass_passed"]) / n, 1),
                "final_success_pct": round(100.0 * sum(1 for r in subset if r["final_passed"]) / n, 1),
                "stale_hallucination_first_pass_pct": round(
                    100.0 * sum(1 for r in subset if r["stale_hallucination_first_pass"]) / n, 1
                ),
                "main_tree_preserved_pct": round(100.0 * sum(1 for r in subset if r["main_tree_preserved"]) / n, 1),
                "wall_clock_mean_s": round(wall_m, 2),
                "wall_clock_sd_s": round(wall_sd, 2),
                "manual_supplements_mean": round(suppl_m, 2),
                "manual_supplements_sd": round(suppl_sd, 2),
                "clarifications_total": sum(r["clarifications"] for r in subset),
                "corrections_total": sum(r["corrections"] for r in subset),
                "ui_approvals_mean": round(ui_m, 2),
                "ui_approvals_sd": round(ui_sd, 2),
                "avg_total_tokens": round(tok_m, 0),
                "effective_time_sensitivity": eff_sensitivity,
            }
        )

        for tid in TASKS:
            tsub = [r for r in subset if r["task_id"] == tid]
            twm, twsd = _mean_sd([r["wall_clock_ms"] / 1000.0 for r in tsub])
            tsm, tssd = _mean_sd([float(r["manual_supplements"]) for r in tsub])
            eff30_vals = [
                (r["wall_clock_ms"] / 1000.0) + r["manual_supplements"] * 30.0 + r["ui_approvals"] * approval_wait_s
                for r in tsub
            ]
            e30m, e30sd = _mean_sd(eff30_vals)
            per_task_summaries.append(
                {
                    "baseline": b,
                    "task_id": tid,
                    "n_trials": len(tsub),
                    "first_pass_pct": round(100.0 * sum(1 for r in tsub if r["first_pass_passed"]) / len(tsub), 1),
                    "final_pct": round(100.0 * sum(1 for r in tsub if r["final_passed"]) / len(tsub), 1),
                    "wall_clock_mean_s": round(twm, 2),
                    "wall_clock_sd_s": round(twsd, 2),
                    "manual_supplements_mean": round(tsm, 2),
                    "manual_supplements_sd": round(tssd, 2),
                    "eff_time_30s_mean_s": round(e30m, 2),
                    "eff_time_30s_sd_s": round(e30sd, 2),
                }
            )

    return {
        "experiment": "track5-baseline-comparison",
        "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "model": model_name,
        "trials_per_task": trials,
        "daemon_bin": sanitize_text(str(daemon_bin.relative_to(REPO_ROOT)) if daemon_bin.is_relative_to(REPO_ROOT) else daemon_bin.name),
        "human_wait_sensitivity_formula": "T_eff(t_h) = T_wall + manual_supplements * t_h + ui_approvals * 5s",
        "baselines": baseline_summaries,
        "per_task": per_task_summaries,
        "sensitivity": sensitivity_rows,
    }


def main() -> int:
    load_dotenv(REPO_ROOT / ".env")
    parser = argparse.ArgumentParser(description="Track 5 Experiment 1: Three-baseline comparison")
    parser.add_argument("--daemon-bin", default=None, help="Path to relay-daemon binary")
    parser.add_argument("--out", default=str(DEFAULT_OUT), help="Output directory (default: course/bench/results)")
    parser.add_argument("--trials", type=int, default=3, help="Number of trials per baseline x task")
    parser.add_argument("--max-turns", type=int, default=12, help="Initial autonomous turn budget")
    parser.add_argument("--max-corrections", type=int, default=3, help="Max deterministic human-oracle correction rounds")
    parser.add_argument("--timeout", type=int, default=180, help="HTTP request timeout in seconds")
    parser.add_argument("--dry-run", action="store_true", help="Verify all 3 baselines x 4 tasks offline without LLM")
    args = parser.parse_args()

    daemon_bin = find_daemon_binary(args.daemon_bin)
    out_dir = Path(args.out).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.dry_run:
        with tempfile.TemporaryDirectory(prefix="relay-baseline-dry-") as tmp:
            for b in BASELINES:
                for tid in TASKS:
                    cdir = Path(tmp) / f"{b}_{tid}"
                    cdir.mkdir(parents=True, exist_ok=True)
                    setup = setup_task_fixture(tid, daemon_bin, cdir, use_relay=(b == "relay_v1"))
                    res = run_dry_baseline_case(b, setup, daemon_bin)
                    print(
                        f"[DRY-PASS] {b:<16} {tid:<22} suppl={res['manual_supplements']} "
                        f"ui={res['ui_approvals']} wall={res['wall_clock_ms']}ms",
                        flush=True,
                    )
        print("[baseline] DRY-RUN PASS: 3/3 baselines x 4/4 tasks verified cleanly.", flush=True)
        return 0

    base_url = os.getenv("RELAY_LLM_BASE_URL", "https://api.deepseek.com").strip()
    model = os.getenv("RELAY_LLM_MODEL", "deepseek-flash").strip()
    key_env = os.getenv("RELAY_LLM_API_KEY_ENV", "RELAY_LLM_API_KEY").strip()
    api_key = None if is_loopback_url(base_url) else (os.getenv(key_env) if key_env else None)
    llm_cfg = {"base_url": base_url, "model": model, "api_key": api_key}

    call_openai_chat(base_url, model, api_key, [{"role": "user", "content": "OK"}], [], timeout_secs=args.timeout, max_tokens=4)
    print(f"[warmup] model={model} ready", flush=True)

    runs: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="relay-baseline-live-") as tmp:
        for trial in range(1, args.trials + 1):
            for b in BASELINES:
                for tid in TASKS:
                    cdir = Path(tmp) / f"t{trial}_{b}_{tid}"
                    cdir.mkdir(parents=True, exist_ok=True)
                    setup = setup_task_fixture(tid, daemon_bin, cdir, use_relay=(b == "relay_v1"))
                    rec = run_live_case(
                        b,
                        setup,
                        daemon_bin,
                        llm_cfg,
                        max_turns=args.max_turns,
                        max_corrections=args.max_corrections,
                        timeout_secs=args.timeout,
                    )
                    rec["trial"] = trial
                    runs.append(rec)
                    status = "PASS" if rec["final_passed"] else "FAIL"
                    print(
                        f"[{status}] t{trial} {b:<16} {tid:<22} 1st={rec['first_pass_passed']} "
                        f"suppl={rec['manual_supplements']}(q={rec['clarifications']},c={rec['corrections']}) "
                        f"ui={rec['ui_approvals']} wall={rec['wall_clock_ms']}ms tok={rec['total_tokens']}",
                        flush=True,
                    )
                    if rec["failures"]:
                        print(f"       failures: {rec['failures']}", flush=True)

    runs_jsonl = out_dir / "baseline_comparison_runs.jsonl"
    with runs_jsonl.open("w", encoding="utf-8", newline="\n") as fh:
        for rec in runs:
            fh.write(json.dumps(rec) + "\n")

    summary = summarize_results(runs, daemon_bin, model, args.trials)
    summary_json = out_dir / "baseline_comparison_summary.json"
    summary_json.write_text(json.dumps(summary, indent=2), encoding="utf-8")

    summary_csv = out_dir / "baseline_comparison_summary.csv"
    with summary_csv.open("w", encoding="utf-8", newline="") as fh:
        rows = summary["per_task"]
        writer = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)

    print(f"[baseline] Saved {runs_jsonl}, {summary_json}, and {summary_csv}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
