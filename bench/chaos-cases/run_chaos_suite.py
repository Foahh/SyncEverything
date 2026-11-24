#!/usr/bin/env python3
"""Relay Track 4 — dual-mode Chaos Suite judge.

Supports two execution modes against the real `relay-daemon` binary and an
isolated Git checkout of `bench/fixture-repo`:

1. `--mode scripted` (default): deterministic offline regression gate across
   all 8 frozen `ChaosCase` fault kinds + an iroh loopback P2P gate
   (`relay-daemon sandbox-dual`). Zero LLM and zero network required.
2. `--mode llm`: drives a live OpenAI-compatible LLM (auto-loaded from `.env`
   via `RELAY_LLM_BASE_URL` / `RELAY_LLM_MODEL` / `RELAY_LLM_API_KEY`) as a
   coding agent equipped with all 8 frozen Relay MCP tools plus 2 repo-confined
   read-only workspace helpers (`list_repo_files`, `read_repo_file`), scoring
   self-healing success rate, tool recall, retry overhead, and STALE immunity
   for Track 5 evaluation.

Usage:
    uv run python bench/chaos-cases/run_chaos_suite.py
    uv run python bench/chaos-cases/run_chaos_suite.py --mode llm
    uv run python bench/chaos-cases/run_chaos_suite.py --case chaos-04-mid-run-disconnect --keep
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import queue
import re
import shutil
import signal
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
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "eval"))
from stale_metrics import cites_stale_forecast_error

FIXTURE_SRC = REPO_ROOT / "bench" / "fixture-repo"
CASES_DIR = Path(__file__).resolve().parent
RESULTS_DIR = CASES_DIR / "results"

VENDOR_WHEEL = "./vendor/tiny_stats-0.1.0-py3-none-any.whl"
CORRUPTED_LOCKFILE = "version = 1\nrevision = 3\n\n[[package]\nname = \n"
TEXT_SUFFIXES = {".py", ".toml", ".lock", ".md", ".txt", ".sh", ".json"}
IGNORE_PATTERNS = shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc", ".DS_Store")

FROZEN_MCP_TOOLS = frozenset(
    {
        "inspect_sync_status",
        "list_handoffs",
        "check_env_diff",
        "get_dag_snapshot",
        "read_evidence_log",
        "run_preflight_in_worktree",
        "propose_worktree_patch",
        "approve_and_execute_task",
    }
)
HEALING_FAULT_KINDS = frozenset({"missing_dependency", "same_line_conflict", "runtime_exception", "corrupted_env_lock"})

LLM_SYSTEM_PROMPT = """You are a coding & experiment agent interacting with Relay over MCP.

You have:
- Relay's 8 MCP tools (`inspect_sync_status`, `list_handoffs`, `check_env_diff`, `get_dag_snapshot`, `read_evidence_log`, `run_preflight_in_worktree`, `propose_worktree_patch`, `approve_and_execute_task`).
- Two read-only workspace inspection helpers: `list_repo_files` and `read_repo_file`.

Mandatory workflow & safety rules:
1. Never invent metrics, commit SHAs, or execution outcomes without tool output.
2. Always call `get_dag_snapshot` before answering questions about experiment metrics. Report numeric results only from fresh evidence for the current code. If all results are stale, explain why and withhold their values. Do NOT retry `read_evidence_log` if it refuses a STALE node.
3. For queued tasks, follow: `run_preflight_in_worktree` -> if smoke fails (or a code fix is requested), inspect relevant files / tracebacks and call `propose_worktree_patch` -> then call `approve_and_execute_task`.
4. The host runs `uv` strictly offline (`UV_OFFLINE=1`). If a module is missing, inspect `list_repo_files` to find any vendored wheel (e.g. `./vendor/...`) and pass that local wheel path to `propose_worktree_patch(patch_kind="uv_spec", ...)`.
5. When calling `propose_worktree_patch(patch_kind="code_diff", ...)`, provide a complete, valid `git diff` unified patch (`diff --git a/<path> b/<path>\\n--- a/<path>\\n+++ b/<path>\\n@@ -start,count +start,count @@\\n...`) ending with a trailing newline (`\\n`). Always inspect the target file with `read_repo_file` first so hunk lines and line counts match verbatim.
6. Respect the human approval gate: call `approve_and_execute_task` WITHOUT `approved_via_ui=true` first so Relay surfaces `approval_required`, unless the user prompt tells you approval was already granted (e.g. a duplicate replay after lost response) or a reviewer approves in a follow-up message.
"""


# ---------------------------------------------------------------------------
# Environment & filesystem helpers
# ---------------------------------------------------------------------------


def load_dotenv(path: Path) -> None:
    """Load KEY=VALUE pairs from `.env` in memory without logging secrets or overriding explicit vars."""
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


def fixture_env(*, rust_log: str = "warn") -> dict[str, str]:
    """Build a subprocess environment with uv locked to deterministic offline mode."""
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


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return run_cmd(["git", "-C", str(repo), *args], check=check)


def commit_all(repo: Path, message: str) -> str:
    git(repo, "add", "--all")
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD").stdout.strip()


def head_sha(repo: Path) -> str:
    return git(repo, "rev-parse", "HEAD").stdout.strip()


def is_dirty(repo: Path) -> bool:
    return git(repo, "status", "--porcelain").stdout.strip() != ""


def normalize_lf(root: Path) -> None:
    """Normalize text files to LF so unified diffs apply identically on Windows and POSIX."""
    for file_path in root.rglob("*"):
        if file_path.is_file() and file_path.suffix.lower() in TEXT_SUFFIXES:
            text = file_path.read_text(encoding="utf-8")
            file_path.write_text(text, encoding="utf-8", newline="\n")


def init_repo(dest: Path) -> None:
    shutil.copytree(FIXTURE_SRC, dest, ignore=IGNORE_PATTERNS)
    normalize_lf(dest)
    (dest / ".gitignore").write_text(".venv/\n.relay/\n__pycache__/\n", encoding="utf-8", newline="\n")
    run_cmd(["git", "init", "--initial-branch=main"], cwd=dest)
    git(dest, "config", "core.autocrlf", "false")
    git(dest, "config", "user.name", "Relay Chaos Suite")
    git(dest, "config", "user.email", "chaos@relay.local")
    commit_all(dest, "chore(bench): initialize forecast fixture")


def baseline_lockfile() -> str:
    return (FIXTURE_SRC / "uv.lock").read_text(encoding="utf-8").replace("\r\n", "\n")


def unified_diff(old: str, new: str, relpath: str) -> str:
    body = "".join(
        difflib.unified_diff(
            old.splitlines(keepends=True),
            new.splitlines(keepends=True),
            fromfile=f"a/{relpath}",
            tofile=f"b/{relpath}",
            n=3,
        )
    )
    if not body:
        raise RuntimeError(f"empty diff for {relpath}")
    return f"diff --git a/{relpath} b/{relpath}\n{body}"


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


# ---------------------------------------------------------------------------
# Confined read-only workspace inspection tools (for --mode llm)
# ---------------------------------------------------------------------------

BLOCKED_PATH_PARTS = {".git", ".relay", ".relay-desktop", ".venv", "__pycache__"}
BLOCKED_SUFFIXES = (".db", ".db-wal", ".db-shm", ".pem", ".key", ".pyc")


def resolve_safe_repo_path(repo: Path, rel_path: str) -> Path:
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
    root = repo.resolve()
    target = (repo / rel).resolve()
    if target != root and root not in target.parents:
        raise ValueError(f"path '{trimmed}' escapes repository root")
    return target


def local_list_repo_files(repo: Path) -> str:
    files: list[str] = []
    for root_str, dirnames, filenames in os.walk(repo):
        dirnames[:] = sorted(d for d in dirnames if d not in BLOCKED_PATH_PARTS and not d.startswith(".env"))
        root_path = Path(root_str)
        for fname in sorted(filenames):
            lower = fname.lower()
            if lower.startswith(".env") or lower.endswith(BLOCKED_SUFFIXES):
                continue
            rel = (root_path / fname).relative_to(repo).as_posix()
            files.append(rel)
    return json.dumps(files)


def local_read_repo_file(repo: Path, path: str, start_line: int = 1, end_line: int = 200) -> str:
    target = resolve_safe_repo_path(repo, path)
    if not target.is_file():
        raise FileNotFoundError(f"file not found: {path}")
    raw = target.read_bytes()
    if b"\x00" in raw:
        raise ValueError(f"refusing to read binary file: {path}")
    lines = raw.decode("utf-8", errors="replace").splitlines()
    start = max(1, int(start_line))
    end = min(len(lines), max(start, int(end_line)))
    numbered = [f"{idx:4d} | {lines[idx - 1]}" for idx in range(start, end + 1)]
    header = f"# {path} (lines {start}-{end} of {len(lines)})"
    return header + "\n" + "\n".join(numbered)


def local_helper_tools_openai() -> list[dict[str, Any]]:
    return [
        {
            "type": "function",
            "function": {
                "name": "list_repo_files",
                "description": "List repository-relative file paths in the active project (read-only).",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
            },
        },
        {
            "type": "function",
            "function": {
                "name": "read_repo_file",
                "description": "Read a UTF-8 file inside the active repository with line numbers (read-only).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Repository-relative path (e.g. src/forecast.py)"},
                        "start_line": {"type": "integer", "description": "1-based start line (default 1)"},
                        "end_line": {"type": "integer", "description": "1-based inclusive end line (default 200)"},
                    },
                    "required": ["path"],
                },
            },
        },
    ]


# ---------------------------------------------------------------------------
# MCP stdio client
# ---------------------------------------------------------------------------


class McpError(RuntimeError):
    pass


class Daemon:
    """JSON-RPC 2.0 client for `relay-daemon mcp-stdio`."""

    def __init__(self, daemon_bin: Path, repo: Path, data: Path) -> None:
        self.proc = subprocess.Popen(
            [
                str(daemon_bin),
                "--repo-root",
                str(repo),
                "--data-dir",
                str(data),
                "mcp-stdio",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            start_new_session=(os.name != "nt"),
            env=fixture_env(),
        )
        self._next_id = 1
        self._queue: queue.Queue[str] = queue.Queue()
        self._stderr: list[str] = []
        self._killed = False
        self.called_tools: list[str] = []
        self.tools_schema: list[dict[str, Any]] = []
        threading.Thread(target=self._pump_stdout, daemon=True).start()
        threading.Thread(target=self._pump_stderr, daemon=True).start()

    def _pump_stdout(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            self._queue.put(line)
        self._queue.put("")

    def _pump_stderr(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            self._stderr.append(line.rstrip())
            if len(self._stderr) > 300:
                self._stderr.pop(0)

    def stderr_tail(self, lines: int = 12) -> str:
        return "\n".join(self._stderr[-lines:])

    def _send(self, message: dict[str, Any]) -> None:
        if self.proc.stdin is None or self.proc.stdin.closed:
            raise McpError("daemon stdin is closed")
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def request(self, method: str, params: dict[str, Any] | None = None, timeout: float = 240.0) -> dict[str, Any]:
        req_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {"jsonrpc": "2.0", "id": req_id, "method": method}
        if params is not None:
            payload["params"] = params
        self._send(payload)
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise McpError(f"timeout waiting for {method}")
            try:
                line = self._queue.get(timeout=remaining)
            except queue.Empty:
                raise McpError(f"timeout waiting for {method}") from None
            if not line:
                raise McpError(f"daemon exited during {method}\nSTDERR:\n{self.stderr_tail()}")
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") != req_id:
                continue
            if "error" in msg:
                raise McpError(f"MCP error for {method}: {msg['error']}")
            return msg["result"]

    def initialize(self) -> None:
        self.request(
            "initialize",
            {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "relay-track4-chaos", "version": "1.0.0"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        listed = self.request("tools/list", {})
        self.tools_schema = listed.get("tools", [])

    def openai_tools(self) -> list[dict[str, Any]]:
        return [
            {
                "type": "function",
                "function": {
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "parameters": tool.get("inputSchema", {"type": "object", "properties": {}}),
                },
            }
            for tool in self.tools_schema
        ]

    def call(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        timeout: float = 240.0,
        *,
        track: bool = True,
    ) -> tuple[bool, Any, str]:
        if track:
            self.called_tools.append(name)
        result = self.request("tools/call", {"name": name, "arguments": arguments or {}}, timeout=timeout)
        is_err = bool(result.get("isError", False))
        text = "\n".join(block.get("text", "") for block in result.get("content", []) if block.get("type") == "text")
        payload: Any = None
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            payload = None
        return is_err, payload, text

    def kill_tree(self) -> None:
        self._killed = True
        if self.proc.poll() is None:
            if os.name == "nt":
                subprocess.run(
                    ["taskkill", "/F", "/T", "/PID", str(self.proc.pid)],
                    capture_output=True,
                    check=False,
                )
            else:
                try:
                    os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if self.proc.poll() is None:
                self.proc.kill()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass

    def close(self) -> None:
        if self._killed:
            return
        if self.proc.poll() is None:
            try:
                if self.proc.stdin:
                    self.proc.stdin.close()
            except OSError:
                pass
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.kill_tree()

    def __enter__(self) -> "Daemon":
        self.initialize()
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()


# ---------------------------------------------------------------------------
# CLI & DB helpers shared by scripted + llm recipes
# ---------------------------------------------------------------------------


def submit_task(
    daemon_bin: Path,
    repo: Path,
    data: Path,
    preset: str,
    params: list[str] | None = None,
    *,
    note: str | None = None,
) -> str:
    cmd = [str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "submit-task", "--preset", preset]
    for param in params or []:
        cmd.extend(["--param", param])
    if note:
        cmd.extend(["--note", note])
    proc = run_cmd(cmd)
    return json.loads(proc.stdout)["spec"]["task_id"]


def sync_head(daemon_bin: Path, repo: Path, data: Path) -> dict[str, Any]:
    proc = run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"])
    return json.loads(proc.stdout)


def compose_bundles(daemon_bin: Path, repo: Path, data: Path) -> dict[str, Any]:
    proc = run_cmd(
        [
            str(daemon_bin),
            "--repo-root",
            str(repo),
            "--data-dir",
            str(data),
            "compose-context",
            "--all-strategies",
            "--no-sync",
        ]
    )
    return json.loads(proc.stdout)


def read_task(data: Path, task_id: str) -> dict[str, Any]:
    db = data / "relay.db"
    if not db.exists():
        raise RuntimeError(f"task database missing at {db}")
    conn = sqlite3.connect(str(db), timeout=10)
    try:
        row = conn.execute(
            "SELECT state, active_proposal_json, preflight_summary, evidence_node_id, error_message "
            "FROM tasks WHERE task_id = ?",
            (task_id,),
        ).fetchone()
    finally:
        conn.close()
    if row is None:
        raise RuntimeError(f"task {task_id} is not persisted")
    return {
        "state": json.loads(row[0]) if row[0] else None,
        "proposal": json.loads(row[1]) if row[1] else None,
        "preflight_summary": row[2],
        "evidence_node_id": row[3],
        "error_message": row[4],
    }


def dag_snapshot(daemon: Daemon, *, track: bool = True) -> dict[str, Any]:
    is_err, payload, text = daemon.call("get_dag_snapshot", {"include_stale": True}, track=track)
    if is_err or payload is None:
        raise RuntimeError(f"get_dag_snapshot failed: {text}")
    return payload


def evidence_stats(snapshot: dict[str, Any]) -> dict[str, int]:
    evidence = [node for node in snapshot["nodes"] if node["payload"]["kind"] == "evidence"]
    fresh = sum(1 for node in evidence if node["freshness"]["status"] == "fresh")
    stale = sum(1 for node in evidence if node["freshness"]["status"] == "stale")
    return {"total": len(evidence), "fresh": fresh, "stale": stale}


def stale_evidence_id(snapshot: dict[str, Any]) -> str | None:
    for node in snapshot["nodes"]:
        if node["payload"]["kind"] == "evidence" and node["freshness"]["status"] == "stale":
            return node["id"]
    return None


def best_mae(snapshot: dict[str, Any]) -> float | None:
    for node in snapshot["nodes"]:
        if node["payload"]["kind"] == "evidence":
            metrics = node["payload"].get("metrics_json") or {}
            value = metrics.get("best_mae")
            if isinstance(value, (int, float)):
                return float(value)
    return None


def best_window(snapshot: dict[str, Any]) -> int | None:
    for node in snapshot["nodes"]:
        if node["payload"]["kind"] == "evidence":
            metrics = node["payload"].get("metrics_json") or {}
            value = metrics.get("best_window")
            if isinstance(value, (int, float)):
                return int(value)
    return None


def stale_error_blocked(is_err: bool, text: str) -> bool:
    return bool(is_err and "stale" in text.lower())


def compute_recall(expected: list[str], called: list[str]) -> float:
    if not expected:
        return 1.0
    called_set = set(called)
    hits = sum(1 for item in expected if item in called_set)
    return round(hits / len(expected), 3)


def check_stale_hallucination(answer: str, snapshot: dict[str, Any], before_edit: dict[str, Any]) -> bool:
    lower = answer.lower()
    acknowledged_stale = any(
        kw in lower
        for kw in ("stale", "outdated", "superseded", "invalid", "re-run", "rerun", "no fresh", "no valid", "过期", "陈旧", "失效", "重跑")
    )
    cites_old_mae = cites_stale_forecast_error(answer, snapshot, before_edit)
    return (not acknowledged_stale) or cites_old_mae


# ---------------------------------------------------------------------------
# P2P loopback connectivity gate (`relay-daemon sandbox-dual`)
# ---------------------------------------------------------------------------


def run_p2p_loopback_gate(daemon_bin: Path, workroot: Path, *, keep: bool, timeout_secs: float = 30.0) -> dict[str, Any]:
    gate_dir = workroot / "_p2p_loopback_gate"
    if gate_dir.exists():
        shutil.rmtree(gate_dir, ignore_errors=True)
    gate_dir.mkdir(parents=True)
    repo = gate_dir / "repo"
    data = gate_dir / "data"
    init_repo(repo)

    started = time.perf_counter()
    proc = subprocess.Popen(
        [str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sandbox-dual"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        start_new_session=(os.name != "nt"),
        env=fixture_env(rust_log="info"),
    )

    lines_q: queue.Queue[str] = queue.Queue()

    def _pump() -> None:
        assert proc.stderr is not None
        for line in proc.stderr:
            lines_q.put(line.rstrip())
        lines_q.put("")

    threading.Thread(target=_pump, daemon=True).start()

    seen_lines: list[str] = []
    passed = False
    rtt_ms: int | None = None
    detail = ""
    deadline = time.monotonic() + timeout_secs
    try:
        while time.monotonic() < deadline:
            remaining = max(0.05, deadline - time.monotonic())
            try:
                line = lines_q.get(timeout=remaining)
            except queue.Empty:
                detail = f"timed out after {timeout_secs:.0f}s waiting for sandbox-dual ping"
                break
            if not line:
                detail = f"sandbox-dual exited prematurely ({proc.returncode})"
                break
            seen_lines.append(line)
            if "Local Host and Client ping succeeded" in line:
                passed = True
                detail = line
                match = re.search(r"rtt_ms=(?:Some\()?(\d+)", line)
                if match:
                    rtt_ms = int(match.group(1))
                break
    finally:
        if proc.poll() is None:
            if os.name == "nt":
                subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)], capture_output=True, check=False)
            else:
                try:
                    os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
        if not keep:
            shutil.rmtree(gate_dir, ignore_errors=True)

    elapsed = round((time.perf_counter() - started) * 1000.0, 1)
    if not passed and seen_lines and "prematurely" in detail:
        detail = f"{detail}: {' | '.join(seen_lines[-4:])}"
    return {
        "passed": passed,
        "rtt_ms": rtt_ms,
        "duration_ms": elapsed,
        "detail": detail or "ok",
    }


# ---------------------------------------------------------------------------
# Scripted Chaos recipes (--mode scripted) — one per frozen InjectedFaultKind
# ---------------------------------------------------------------------------


def scripted_missing_dependency(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    forecast = repo / "src" / "forecast.py"
    write_text(forecast, read_text(forecast) + "\nimport tiny_stats  # noqa: F401\n")
    commit_all(repo, "feat(bench): probe an undeclared vendored dependency")

    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        is_err, env_diff, text = daemon.call("check_env_diff", {"commit_sha": "HEAD"})
        if is_err or env_diff is None:
            raise RuntimeError(f"check_env_diff failed: {text}")

        is_err, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if is_err or report is None or report.get("smoke_passed", True):
            raise RuntimeError(f"preflight should fail with a missing module: {text}")
        metrics["detected_missing_module"] = report.get("detected_missing_module")

        is_err, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "uv_spec",
                "summary": "Declare tiny_stats from the offline vendor wheel",
                "content": VENDOR_WHEEL,
            },
        )
        if is_err or proposal is None:
            raise RuntimeError(f"uv_spec proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]

        # Trigger the genuine approval gate BEFORE setting approved_via_ui=True.
        is_err, gate, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"]},
        )
        if is_err or gate is None:
            raise RuntimeError(f"approval gate call failed: {text}")
        pre_gate_stats = evidence_stats(dag_snapshot(daemon, track=False))
        metrics["approval_required_observed"] = (
            gate.get("outcome") == "approval_required"
            and bool(gate.get("diff_preview"))
            and read_task(data, task_id)["state"] == "self_healing_proposed"
            and pre_gate_stats["total"] == 0
        )

        is_err, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if is_err or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon, track=False)
        called_tools = list(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_count": stats["total"],
            "evidence_fresh_count": stats["fresh"],
            "stale_evidence_count": stats["stale"],
            "main_worktree_dirty": is_dirty(repo),
            "best_mae": best_mae(snapshot),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_upstream_edit(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    called_tools: list[str] = []
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        is_err, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if is_err or response is None:
            raise RuntimeError(f"baseline execution failed: {text}")
        called_tools.extend(daemon.called_tools)

    forecast = repo / "src" / "forecast.py"
    write_text(
        forecast,
        read_text(forecast).replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=0.5, help="EWMA smoothing factor")',
        ),
    )
    commit_all(repo, "feat(bench): change default smoothing alpha after the host run")
    head_sync = sync_head(daemon_bin, repo, data)

    metrics: dict[str, Any] = {"sync_head_status": head_sync.get("status")}
    with Daemon(daemon_bin, repo, data) as daemon:
        snapshot = dag_snapshot(daemon)
        stale_id = stale_evidence_id(snapshot)
        metrics["stale_read_blocked"] = False
        if stale_id:
            is_err, _, text = daemon.call("read_evidence_log", {"node_id": stale_id})
            metrics["stale_read_blocked"] = stale_error_blocked(is_err, text)
        called_tools.extend(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_count": stats["total"],
            "evidence_fresh_count": stats["fresh"],
            "stale_evidence_count": stats["stale"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_same_line_conflict(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin, bindir = ctx["repo"], ctx["data"], ctx["daemon_bin"], ctx["bindir"]
    client = bindir / "laptop-checkout"
    shutil.copytree(repo, client, ignore=shutil.ignore_patterns(".venv", "__pycache__"))
    client_forecast = client / "src" / "forecast.py"
    write_text(client_forecast, read_text(client_forecast).replace("default=0.35", "default=0.55", 1))
    before_bytes = client_forecast.read_bytes()

    forecast = repo / "src" / "forecast.py"
    patched = read_text(forecast).replace(
        'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
        'parser.add_argument("--alpha", type=float, default=0.45, help="EWMA smoothing factor")',
    )
    patch = unified_diff(read_text(forecast), patched, "src/forecast.py")

    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        is_err, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Raise the default EWMA alpha to 0.45",
                "content": patch,
            },
        )
        if is_err or proposal is None:
            raise RuntimeError(f"code_diff proposal failed: {text}")
        is_err, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if is_err or response is None:
            raise RuntimeError(f"host execution failed: {text}")
        snapshot = dag_snapshot(daemon, track=False)
        diff_preview = proposal["diff_preview"]
        called_tools = list(daemon.called_tools)

    patch_file = bindir / "host-patch.diff"
    patch_file.write_text(diff_preview, encoding="utf-8", newline="\n")
    # Real `git apply` (not --check): must fail cleanly without touching the laptop's dirty edit.
    apply_res = git(client, "apply", "--whitespace=nowarn", str(patch_file), check=False)
    metrics["conflict_apply_failed"] = apply_res.returncode != 0
    metrics["client_edit_preserved"] = (
        client_forecast.read_bytes() == before_bytes and "default=0.55" in read_text(client_forecast)
    )
    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_fresh_count": stats["fresh"],
            "stale_evidence_count": stats["stale"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def setup_mid_run_disconnect(repo: Path, data: Path, daemon_bin: Path) -> tuple[str, list[str], str]:
    forecast = repo / "src" / "forecast.py"
    write_text(
        forecast,
        read_text(forecast).replace(
            "    t0 = time.perf_counter()\n    series = generate_series(80 if smoke else 360)",
            "    if smoke:\n        time.sleep(6)\n    t0 = time.perf_counter()\n"
            "    series = generate_series(80 if smoke else 360)",
        ),
    )
    commit_all(repo, "feat(bench): slow the smoke path to open a disconnect window")
    task_id = submit_task(daemon_bin, repo, data, "train_grid", note="Slow smoke disconnect recovery")
    short_id = task_id.replace("-", "")[16:]
    worktree_forecast = data / "worktrees" / short_id / "src" / "forecast.py"
    worktree_lock = repo / ".git" / "worktrees" / short_id / "locked"

    first = Daemon(daemon_bin, repo, data)
    first.initialize()

    def preflight() -> None:
        try:
            first.call("run_preflight_in_worktree", {"task_id": task_id}, timeout=120)
        except McpError:
            pass

    worker = threading.Thread(target=preflight, daemon=True)
    worker.start()
    # Wait until SQLite records preflight_running AND `git worktree add` finishes (unlocked),
    # so we disconnect during `uv run --smoke` rather than corrupting git's worktree-add lock.
    deadline = time.monotonic() + 20.0
    observed_running = False
    while time.monotonic() < deadline:
        if (
            read_task(data, task_id)["state"] == "preflight_running"
            and worktree_forecast.is_file()
            and not worktree_lock.exists()
        ):
            observed_running = True
            break
        time.sleep(0.05)
    if not observed_running:
        first.kill_tree()
        raise RuntimeError("task did not enter unlocked smoke preflight window before timeout")

    time.sleep(0.8)
    first.kill_tree()
    worker.join(timeout=10)
    time.sleep(0.4)
    crash_state = read_task(data, task_id)["state"]
    return task_id, list(first.called_tools), crash_state


def scripted_mid_run_disconnect(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id, first_calls, crash_state = setup_mid_run_disconnect(repo, data, daemon_bin)
    metrics: dict[str, Any] = {"crash_state_persisted": crash_state}
    with Daemon(daemon_bin, repo, data) as daemon:
        is_err, sync_status, text = daemon.call("inspect_sync_status", {})
        if is_err or sync_status is None:
            raise RuntimeError(f"inspect_sync_status failed: {text}")
        is_err, handoffs, text = daemon.call("list_handoffs", {"limit": 10})
        if is_err or not isinstance(handoffs, list) or not handoffs:
            raise RuntimeError(f"list_handoffs failed or empty after reconnect: {text}")

        is_err, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id}, timeout=240)
        if is_err or report is None:
            raise RuntimeError(f"resumed preflight failed: {text}\n--\n{daemon.stderr_tail()}")
        metrics["resume_preflight_passed"] = bool(report["smoke_passed"])

        is_err, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if is_err or response is None:
            raise RuntimeError(f"post-reconnect execution failed: {text}")
        snapshot = dag_snapshot(daemon, track=False)
        called_tools = first_calls + list(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_count": stats["total"],
            "evidence_fresh_count": stats["fresh"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_runtime_exception(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    forecast = repo / "src" / "forecast.py"
    broken = read_text(forecast).replace(
        'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
        'parser.add_argument("--alpha", type=float, default=DEFAULT_ALPHA, help="EWMA smoothing factor")',
    )
    write_text(forecast, broken)
    commit_all(repo, "feat(bench): reference an undefined DEFAULT_ALPHA")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")

    fixed = broken.replace(
        "from typing import Iterable\n",
        "from typing import Iterable\n\nDEFAULT_ALPHA = 0.35\n",
    )
    patch = unified_diff(broken, fixed, "src/forecast.py")

    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        is_err, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if is_err or report is None:
            raise RuntimeError(f"preflight call errored unexpectedly: {text}")
        raw_err = (report.get("stderr_tail") or "") + (report.get("raw_log") or "")
        metrics["preflight_name_error_detected"] = (not report.get("smoke_passed", True)) and ("NameError" in raw_err)
        metrics["preflight_error"] = (report.get("stderr_tail") or "").strip().splitlines()[-1:]

        is_err, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Define DEFAULT_ALPHA = 0.35 before the argparse default is evaluated",
                "content": patch,
            },
        )
        if is_err or proposal is None:
            raise RuntimeError(f"code_diff proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]

        is_err, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if is_err or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon, track=False)
        called_tools = list(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_fresh_count": stats["fresh"],
            "main_worktree_dirty": is_dirty(repo),
            "host_metrics_best_window": best_window(snapshot),
            "host_metrics_best_mae": best_mae(snapshot),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_corrupted_lock(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    write_text(repo / "uv.lock", CORRUPTED_LOCKFILE)
    commit_all(repo, "fix(bench): merge a truncated uv.lock")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    patch = unified_diff(CORRUPTED_LOCKFILE, baseline_lockfile(), "uv.lock")

    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        is_err, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if is_err or report is None:
            raise RuntimeError(f"preflight call errored unexpectedly: {text}")
        stderr = ((report.get("stderr_tail") or "") + "\n" + (report.get("raw_log") or "")).lower()
        metrics["uv_lock_parse_failed"] = (
            (not report.get("smoke_passed", True))
            and ("uv.lock" in stderr)
            and any(tok in stderr for tok in ("parse", "toml", "invalid", "expected"))
        )

        is_err, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Restore a valid uv.lock so the offline environment can start",
                "content": patch,
            },
        )
        if is_err or proposal is None:
            raise RuntimeError(f"lockfile repair proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]

        is_err, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if is_err or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon, track=False)
        called_tools = list(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_fresh_count": stats["fresh"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_stale_evidence_trap(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id}, track=False)
        is_err, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}, track=False
        )
        if is_err or response is None:
            raise RuntimeError(f"baseline execution failed: {text}")

    forecast = repo / "src" / "forecast.py"
    write_text(
        forecast,
        read_text(forecast).replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=0.6, help="EWMA smoothing factor")',
        ),
    )
    commit_all(repo, "refactor(bench): retune the smoothing default")
    sync_head(daemon_bin, repo, data)

    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        snapshot = dag_snapshot(daemon)
        stale_id = stale_evidence_id(snapshot)
        if stale_id is None:
            raise RuntimeError("expected a STALE evidence node after the upstream edit")
        is_err, _, text = daemon.call("read_evidence_log", {"node_id": stale_id})
        metrics["stale_read_blocked"] = stale_error_blocked(is_err, text)
        called_tools = list(daemon.called_tools)

    bundles = compose_bundles(daemon_bin, repo, data)
    metrics["dag_bundle_stale_metrics"] = int(bundles["dag_bundle"]["exposed_stale_metrics_count"])
    metrics["full_transcript_stale_metrics"] = int(bundles["full_transcript"]["exposed_stale_metrics_count"])
    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "evidence_fresh_count": stats["fresh"],
            "stale_evidence_count": stats["stale"],
            "task_state_final": read_task(data, task_id)["state"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


def scripted_duplicate_replay(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        is_err, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if is_err or response is None:
            raise RuntimeError(f"initial execution failed: {text}")
        first_evidence = read_task(data, task_id)["evidence_node_id"]

        is_err, replay, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if is_err or replay is None:
            raise RuntimeError(f"replayed approval failed: {text}")
        metrics["replay_response"] = replay.get("outcome")
        snapshot = dag_snapshot(daemon)
        called_tools = list(daemon.called_tools)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "replay_evidence_count": stats["total"],
            "evidence_node_id_stable": read_task(data, task_id)["evidence_node_id"] == first_evidence,
            "task_state_final": read_task(data, task_id)["state"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return {"metrics": metrics, "called_tools": called_tools}


SCRIPTED_RECIPES = {
    "missing_dependency": scripted_missing_dependency,
    "upstream_code_edit": scripted_upstream_edit,
    "same_line_conflict": scripted_same_line_conflict,
    "mid_run_disconnect": scripted_mid_run_disconnect,
    "runtime_exception": scripted_runtime_exception,
    "corrupted_env_lock": scripted_corrupted_lock,
    "stale_evidence_trap": scripted_stale_evidence_trap,
    "duplicate_task_replay": scripted_duplicate_replay,
}


# ---------------------------------------------------------------------------
# Live LLM execution engine (--mode llm)
# ---------------------------------------------------------------------------


def is_loopback_url(url: str) -> bool:
    host = (urllib.parse.urlparse(url).hostname or "").lower()
    return host in {"127.0.0.1", "localhost", "::1"}


def call_openai_chat(
    base_url: str,
    model: str,
    api_key: str | None,
    messages: list[dict[str, Any]],
    tools: list[dict[str, Any]],
    *,
    reasoning_effort: str | None = None,
    timeout_secs: int = 240,
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
    if reasoning_effort:
        body["reasoning_effort"] = reasoning_effort
    if max_tokens is not None:
        body["max_tokens"] = max_tokens
    headers = {"Content-Type": "application/json", "User-Agent": "Relay-Chaos-Judge/1.0"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    req = urllib.request.Request(url, data=json.dumps(body).encode("utf-8"), headers=headers, method="POST")
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as err:
        detail = err.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {err.code} from {url}: {detail[:400]}") from err
    elapsed_ms = (time.perf_counter() - t0) * 1000.0
    return json.loads(raw), elapsed_ms


def warmup_llm(llm_cfg: dict[str, Any]) -> None:
    _, elapsed = call_openai_chat(
        llm_cfg["base_url"],
        llm_cfg["model"],
        llm_cfg["api_key"],
        [{"role": "user", "content": "OK"}],
        [],
        reasoning_effort=llm_cfg.get("reasoning_effort"),
        timeout_secs=int(llm_cfg.get("timeout_secs", 240)),
        max_tokens=4,
    )
    print(f"[warmup] model={llm_cfg['model']} ready in {elapsed:.1f}ms", flush=True)


def run_llm_agent_loop(
    daemon: Daemon,
    repo: Path,
    user_prompt: str,
    llm_cfg: dict[str, Any],
    *,
    allow_human_approval: bool,
    previously_approved_task: str | None = None,
) -> dict[str, Any]:
    tools = daemon.openai_tools() + local_helper_tools_openai()
    messages: list[dict[str, Any]] = [
        {"role": "system", "content": LLM_SYSTEM_PROMPT},
        {"role": "user", "content": user_prompt},
    ]

    called_tools: list[str] = []
    local_tools: list[str] = []
    seen_calls: dict[str, int] = {}
    approved_keys = {(previously_approved_task, None)} if previously_approved_task else set()
    tool_events: list[dict[str, Any]] = []
    turns = 0
    retries = 0
    loop_aborted = False
    prompt_tokens = 0
    completion_tokens = 0
    llm_latency_ms = 0.0
    approval_required_observed = False
    final_answer = ""

    for turn in range(1, int(llm_cfg["max_turns"]) + 1):
        turns = turn
        request_started = time.perf_counter()
        try:
            response, elapsed = call_openai_chat(
                llm_cfg["base_url"],
                llm_cfg["model"],
                llm_cfg["api_key"],
                messages,
                tools,
                reasoning_effort=llm_cfg.get("reasoning_effort"),
                timeout_secs=int(llm_cfg.get("timeout_secs", 240)),
            )
        except (TimeoutError, urllib.error.URLError, RuntimeError) as exc:
            llm_latency_ms += (time.perf_counter() - request_started) * 1000.0
            loop_aborted = True
            final_answer = f"[LLM_ERROR] {exc}"
            return {
                "called_tools": called_tools,
                "local_tools": local_tools,
                "turns": turns,
                "retries": retries,
                "loop_aborted": True,
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
                "latency_ms": round(llm_latency_ms, 1),
                "approval_required_observed": approval_required_observed,
                "final_answer": final_answer,
                "tool_events": tool_events,
            }
        llm_latency_ms += elapsed
        usage = response.get("usage") or {}
        prompt_tokens += int(usage.get("prompt_tokens", 0))
        completion_tokens += int(usage.get("completion_tokens", 0))

        choice = response["choices"][0]["message"]
        tool_calls = choice.get("tool_calls") or []
        norm_calls: list[dict[str, Any]] = []
        for idx, tc in enumerate(tool_calls):
            tc_copy = dict(tc)
            if not tc_copy.get("id"):
                tc_copy["id"] = f"call_{turn}_{idx}"
            norm_calls.append(tc_copy)
        assistant_msg: dict[str, Any] = dict(choice)
        assistant_msg["content"] = choice.get("content") or ""
        if norm_calls:
            assistant_msg["tool_calls"] = norm_calls
        messages.append(assistant_msg)
        if not norm_calls:
            final_answer = (choice.get("content") or "").strip()
            break

        pending_approval_msg: str | None = None
        pending_approval_key: tuple[str, str | None] | None = None
        for tc in norm_calls:
            fn = tc["function"]
            name = fn["name"]
            raw_args = fn.get("arguments") or "{}"
            try:
                parsed_args = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
                if not isinstance(parsed_args, dict):
                    parsed_args = {}
            except json.JSONDecodeError:
                parsed_args = {}

            sig = f"{name}:{json.dumps(parsed_args, sort_keys=True)}"
            seen_calls[sig] = seen_calls.get(sig, 0) + 1
            if seen_calls[sig] >= 3:
                loop_aborted = True
                final_answer = f"[LOOP_ABORTED] Repeated identical tool call: {name}"
                return {
                    "called_tools": called_tools,
                    "local_tools": local_tools,
                    "turns": turns,
                    "retries": retries,
                    "loop_aborted": True,
                    "prompt_tokens": prompt_tokens,
                    "completion_tokens": completion_tokens,
                    "total_tokens": prompt_tokens + completion_tokens,
                    "latency_ms": round(llm_latency_ms, 1),
                    "approval_required_observed": approval_required_observed,
                    "final_answer": final_answer,
                    "tool_events": tool_events,
                }

            if name == "list_repo_files":
                local_tools.append(name)
                try:
                    output_text = local_list_repo_files(repo)
                    is_err, payload = False, None
                except Exception as exc:  # noqa: BLE001
                    output_text = f"error: {exc}"
                    is_err, payload = True, None
                    retries += 1
            elif name == "read_repo_file":
                local_tools.append(name)
                try:
                    output_text = local_read_repo_file(
                        repo,
                        str(parsed_args.get("path", "")),
                        int(parsed_args.get("start_line", 1)),
                        int(parsed_args.get("end_line", 200)),
                    )
                    is_err, payload = False, None
                except Exception as exc:  # noqa: BLE001
                    output_text = f"error: {exc}"
                    is_err, payload = True, None
                    retries += 1
            else:
                called_tools.append(name)
                try:
                    if name == "approve_and_execute_task" and parsed_args.get("approved_via_ui"):
                        task_id, patch_id = parsed_args.get("task_id"), parsed_args.get("patch_id")
                        if not isinstance(task_id, str) or not isinstance(patch_id, (str, type(None))):
                            raise McpError("Expected task_id and optional patch_id strings.")
                        if (task_id, patch_id) not in approved_keys:
                            raise McpError("No reviewer approval for this task and patch. Request approval first.")
                    is_err, payload, output_text = daemon.call(name, parsed_args, track=False)
                except McpError as exc:
                    is_err, payload, output_text = True, None, f"MCP error: {exc}"
                if is_err and name != "read_evidence_log":
                    retries += 1
                elif name == "propose_worktree_patch" and isinstance(payload, dict) and not payload.get("smoke_verified", True):
                    retries += 1

                if (
                    name == "approve_and_execute_task"
                    and not is_err
                    and isinstance(payload, dict)
                    and payload.get("outcome") == "approval_required"
                ):
                    approval_required_observed = True
                    gate_key = (payload["task_id"], payload.get("patch_id"))
                    if allow_human_approval and gate_key not in approved_keys:
                        pending_approval_key = gate_key
                        tid = payload.get("task_id", "")
                        pid = payload.get("patch_id")
                        if pid:
                            pending_approval_msg = (
                                f"Reviewer action: Approved in Relay UI. Call `approve_and_execute_task` now with "
                                f"`task_id={tid!r}`, `patch_id={pid!r}`, and `approved_via_ui=true`, then inspect the "
                                "DAG / summarize the verified outcome."
                            )
                        else:
                            pending_approval_msg = (
                                f"Reviewer action: Approved in Relay UI. Call `approve_and_execute_task` now with "
                                f"`task_id={tid!r}` and `approved_via_ui=true`, then inspect the DAG / summarize the verified outcome."
                            )

            tool_events.append({"name": name, "args": parsed_args, "is_err": is_err, "payload": payload, "text": output_text})
            messages.append({"role": "tool", "tool_call_id": tc["id"], "content": output_text})

        if pending_approval_msg is not None and pending_approval_key is not None:
            approved_keys.add(pending_approval_key)
            messages.append({"role": "user", "content": pending_approval_msg})
    else:
        loop_aborted = True
        final_answer = "[MAX_TURNS_EXCEEDED]"

    return {
        "called_tools": called_tools,
        "local_tools": local_tools,
        "turns": turns,
        "retries": retries,
        "loop_aborted": loop_aborted,
        "prompt_tokens": prompt_tokens,
        "completion_tokens": completion_tokens,
        "total_tokens": prompt_tokens + completion_tokens,
        "latency_ms": round(llm_latency_ms, 1),
        "approval_required_observed": approval_required_observed,
        "final_answer": final_answer,
        "tool_events": tool_events,
    }


def extract_tool_outcome(tool_events: list[dict[str, Any]], tool_name: str) -> dict[str, Any] | None:
    for ev in tool_events:
        if ev["name"] == tool_name and not ev["is_err"] and isinstance(ev["payload"], dict):
            return ev["payload"]
    return None


def llm_missing_dependency(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    forecast = repo / "src" / "forecast.py"
    write_text(forecast, read_text(forecast) + "\nimport tiny_stats  # noqa: F401\n")
    commit = commit_all(repo, "feat(bench): probe an undeclared vendored dependency")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")

    prompt = (
        f"Active repository HEAD: {commit}\n"
        f"Queued Task ID: {task_id}\n"
        f"User request: {case['user_prompt']}\n"
        "Use Relay's tools (including check_env_diff, run_preflight_in_worktree, list_repo_files, propose_worktree_patch, and approve_and_execute_task)."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=True)
        snapshot = dag_snapshot(daemon, track=False)

    preflight = extract_tool_outcome(agent_run["tool_events"], "run_preflight_in_worktree") or {}
    task_row = read_task(data, task_id)
    proposal = task_row["proposal"] or {}
    stats = evidence_stats(snapshot)
    metrics = {
        "detected_missing_module": preflight.get("detected_missing_module"),
        "proposal_smoke_verified": bool(proposal.get("smoke_verified")),
        "approval_required_observed": bool(agent_run["approval_required_observed"]),
        "task_state_final": task_row["state"],
        "evidence_count": stats["total"],
        "evidence_fresh_count": stats["fresh"],
        "stale_evidence_count": stats["stale"],
        "main_worktree_dirty": is_dirty(repo),
        "best_mae": best_mae(snapshot),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": case["expected_tool_trace"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


def llm_upstream_edit(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as setup_daemon:
        setup_daemon.call("run_preflight_in_worktree", {"task_id": task_id}, track=False)
        is_err, resp, text = setup_daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}, track=False
        )
        if is_err or resp is None:
            raise RuntimeError(f"baseline setup failed: {text}")
        before_edit = dag_snapshot(setup_daemon, track=False)

    forecast = repo / "src" / "forecast.py"
    write_text(
        forecast,
        read_text(forecast).replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=0.5, help="EWMA smoothing factor")',
        ),
    )
    head_b = commit_all(repo, "feat(bench): change default smoothing alpha after the host run")
    sync_head(daemon_bin, repo, data)

    prompt = (
        f"Active repository HEAD: {head_b}\n"
        f"Prior host task ID: {task_id}\n"
        f"User request: {case['user_prompt']}\n"
        "Inspect the DAG snapshot (and read_evidence_log only if needed) to verify what is valid vs STALE."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=False)
        snapshot = dag_snapshot(daemon, track=False)
        stale_ev = next(
            (
                ev
                for ev in agent_run["tool_events"]
                if ev["name"] == "read_evidence_log" and stale_error_blocked(ev["is_err"], ev["text"])
            ),
            None,
        )
        stale_read_blocked = stale_ev is not None
        if not stale_read_blocked:
            stale_id = stale_evidence_id(snapshot)
            if stale_id:
                is_err, _, text = daemon.call("read_evidence_log", {"node_id": stale_id}, track=False)
                stale_read_blocked = stale_error_blocked(is_err, text)

    stats = evidence_stats(snapshot)
    metrics = {
        "task_state_final": read_task(data, task_id)["state"],
        "evidence_count": stats["total"],
        "evidence_fresh_count": stats["fresh"],
        "stale_evidence_count": stats["stale"],
        "stale_read_blocked": stale_read_blocked,
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": ["get_dag_snapshot"],
        "agent_run": agent_run,
        "stale_hallucination": check_stale_hallucination(agent_run["final_answer"], snapshot, before_edit),
    }


def llm_same_line_conflict(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin, bindir = ctx["repo"], ctx["data"], ctx["daemon_bin"], ctx["bindir"]
    client = bindir / "laptop-checkout"
    shutil.copytree(repo, client, ignore=shutil.ignore_patterns(".venv", "__pycache__"))
    client_forecast = client / "src" / "forecast.py"
    write_text(client_forecast, read_text(client_forecast).replace("default=0.35", "default=0.55", 1))
    before_bytes = client_forecast.read_bytes()

    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    prompt = (
        f"Queued Task ID: {task_id}\n"
        f"Active repository HEAD: {head_sha(repo)}\n"
        f"User request: {case['user_prompt']}\n"
        "On the host, prepare and execute a `code_diff` patch for `src/forecast.py` changing the `--alpha` argparse "
        "`default=0.35` to `default=0.45`, verify smoke preflight, respect human approval, and run `train_grid` in the isolated worktree."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=True)
        snapshot = dag_snapshot(daemon, track=False)

    proposal = read_task(data, task_id)["proposal"] or {}
    diff_preview = proposal.get("diff_preview") or ""
    conflict_apply_failed = False
    if diff_preview.strip():
        patch_file = bindir / "host-patch.diff"
        patch_file.write_text(diff_preview, encoding="utf-8", newline="\n")
        apply_res = git(client, "apply", "--whitespace=nowarn", str(patch_file), check=False)
        conflict_apply_failed = apply_res.returncode != 0

    stats = evidence_stats(snapshot)
    metrics = {
        "conflict_apply_failed": conflict_apply_failed,
        "client_edit_preserved": client_forecast.read_bytes() == before_bytes and "default=0.55" in read_text(client_forecast),
        "task_state_final": read_task(data, task_id)["state"],
        "evidence_fresh_count": stats["fresh"],
        "stale_evidence_count": stats["stale"],
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": case["expected_tool_trace"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


def llm_mid_run_disconnect(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id, _, crash_state = setup_mid_run_disconnect(repo, data, daemon_bin)
    prompt = (
        f"Recovered Task ID: {task_id}\n"
        f"User request: {case['user_prompt']}\n"
        "The host daemon just restarted after the disconnect. Call `inspect_sync_status` and `list_handoffs` to confirm "
        "state survived, resume `run_preflight_in_worktree`, and finish `approve_and_execute_task` once approved."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=True)
        snapshot = dag_snapshot(daemon, track=False)

    preflight = extract_tool_outcome(agent_run["tool_events"], "run_preflight_in_worktree") or {}
    stats = evidence_stats(snapshot)
    metrics = {
        "crash_state_persisted": crash_state,
        "resume_preflight_passed": bool(preflight.get("smoke_passed")),
        "task_state_final": read_task(data, task_id)["state"],
        "evidence_count": stats["total"],
        "evidence_fresh_count": stats["fresh"],
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": case["expected_tool_trace"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


def llm_runtime_exception(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    forecast = repo / "src" / "forecast.py"
    broken = read_text(forecast).replace(
        'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
        'parser.add_argument("--alpha", type=float, default=DEFAULT_ALPHA, help="EWMA smoothing factor")',
    )
    write_text(forecast, broken)
    commit = commit_all(repo, "feat(bench): reference an undefined DEFAULT_ALPHA")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")

    prompt = (
        f"Active repository HEAD: {commit}\n"
        f"Queued Task ID: {task_id}\n"
        f"User request: {case['user_prompt']}\n"
        "Inspect `src/forecast.py` with `read_repo_file` after preflight fails, and define `DEFAULT_ALPHA = 0.35` via `code_diff`."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=True)
        snapshot = dag_snapshot(daemon, track=False)

    preflight = extract_tool_outcome(agent_run["tool_events"], "run_preflight_in_worktree") or {}
    raw_err = (preflight.get("stderr_tail") or "") + (preflight.get("raw_log") or "")
    task_row = read_task(data, task_id)
    proposal = task_row["proposal"] or {}
    stats = evidence_stats(snapshot)
    metrics = {
        "preflight_name_error_detected": (not preflight.get("smoke_passed", True)) and ("NameError" in raw_err),
        "proposal_smoke_verified": bool(proposal.get("smoke_verified")),
        "task_state_final": task_row["state"],
        "evidence_fresh_count": stats["fresh"],
        "main_worktree_dirty": is_dirty(repo),
        "host_metrics_best_window": best_window(snapshot),
        "host_metrics_best_mae": best_mae(snapshot),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": case["expected_tool_trace"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


def llm_corrupted_lock(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    write_text(repo / "uv.lock", CORRUPTED_LOCKFILE)
    commit = commit_all(repo, "fix(bench): merge a truncated uv.lock")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")

    prompt = (
        f"Active repository HEAD: {commit}\n"
        f"Queued Task ID: {task_id}\n"
        f"User request: {case['user_prompt']}\n"
        "Run preflight first, inspect `uv.lock` and `pyproject.toml` with `read_repo_file`, propose a `code_diff` restoring a valid `uv.lock`, and run after approval."
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=True)
        snapshot = dag_snapshot(daemon, track=False)

    preflight = extract_tool_outcome(agent_run["tool_events"], "run_preflight_in_worktree") or {}
    stderr = ((preflight.get("stderr_tail") or "") + "\n" + (preflight.get("raw_log") or "")).lower()
    task_row = read_task(data, task_id)
    proposal = task_row["proposal"] or {}
    stats = evidence_stats(snapshot)
    metrics = {
        "uv_lock_parse_failed": (not preflight.get("smoke_passed", True))
        and ("uv.lock" in stderr)
        and any(tok in stderr for tok in ("parse", "toml", "invalid", "expected")),
        "proposal_smoke_verified": bool(proposal.get("smoke_verified")),
        "task_state_final": task_row["state"],
        "evidence_fresh_count": stats["fresh"],
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": case["expected_tool_trace"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


def llm_stale_evidence_trap(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as setup_daemon:
        setup_daemon.call("run_preflight_in_worktree", {"task_id": task_id}, track=False)
        is_err, resp, text = setup_daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}, track=False
        )
        if is_err or resp is None:
            raise RuntimeError(f"baseline setup failed: {text}")
        before_edit = dag_snapshot(setup_daemon, track=False)

    forecast = repo / "src" / "forecast.py"
    write_text(
        forecast,
        read_text(forecast).replace(
            'parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")',
            'parser.add_argument("--alpha", type=float, default=0.6, help="EWMA smoothing factor")',
        ),
    )
    head_b = commit_all(repo, "refactor(bench): retune the smoothing default")
    sync_head(daemon_bin, repo, data)

    prompt = (
        f"Active repository HEAD: {head_b}\n"
        f"User request: {case['user_prompt']}"
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(daemon, repo, prompt, llm_cfg, allow_human_approval=False)
        snapshot = dag_snapshot(daemon, track=False)
        stale_ev = next(
            (
                ev
                for ev in agent_run["tool_events"]
                if ev["name"] == "read_evidence_log" and stale_error_blocked(ev["is_err"], ev["text"])
            ),
            None,
        )
        stale_read_blocked = stale_ev is not None
        if not stale_read_blocked:
            stale_id = stale_evidence_id(snapshot)
            if stale_id:
                is_err, _, text = daemon.call("read_evidence_log", {"node_id": stale_id}, track=False)
                stale_read_blocked = stale_error_blocked(is_err, text)

    bundles = compose_bundles(daemon_bin, repo, data)
    stats = evidence_stats(snapshot)
    metrics = {
        "stale_read_blocked": stale_read_blocked,
        "dag_bundle_stale_metrics": int(bundles["dag_bundle"]["exposed_stale_metrics_count"]),
        "full_transcript_stale_metrics": int(bundles["full_transcript"]["exposed_stale_metrics_count"]),
        "evidence_fresh_count": stats["fresh"],
        "stale_evidence_count": stats["stale"],
        "task_state_final": read_task(data, task_id)["state"],
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": ["get_dag_snapshot"],
        "agent_run": agent_run,
        "stale_hallucination": check_stale_hallucination(agent_run["final_answer"], snapshot, before_edit),
    }


def llm_duplicate_replay(ctx: dict[str, Any], case: dict[str, Any], llm_cfg: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as setup_daemon:
        setup_daemon.call("run_preflight_in_worktree", {"task_id": task_id}, track=False)
        is_err, resp, text = setup_daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}, track=False
        )
        if is_err or resp is None:
            raise RuntimeError(f"baseline setup failed: {text}")
        first_evidence = read_task(data, task_id)["evidence_node_id"]

    prompt = (
        f"Previously approved Task ID: {task_id} (approval was already granted in UI with `approved_via_ui=true`, "
        "but the client lost the response).\n"
        f"User request: {case['user_prompt']}"
    )
    with Daemon(daemon_bin, repo, data) as daemon:
        agent_run = run_llm_agent_loop(
            daemon, repo, prompt, llm_cfg, allow_human_approval=True, previously_approved_task=task_id
        )
        snapshot = dag_snapshot(daemon, track=False)

    replay = extract_tool_outcome(agent_run["tool_events"], "approve_and_execute_task") or {}
    stats = evidence_stats(snapshot)
    metrics = {
        "replay_response": replay.get("outcome"),
        "replay_evidence_count": stats["total"],
        "evidence_node_id_stable": read_task(data, task_id)["evidence_node_id"] == first_evidence,
        "task_state_final": read_task(data, task_id)["state"],
        "main_worktree_dirty": is_dirty(repo),
    }
    return {
        "metrics": metrics,
        "called_tools": agent_run["called_tools"],
        "expected_llm_tools": ["approve_and_execute_task", "get_dag_snapshot"],
        "agent_run": agent_run,
        "stale_hallucination": False,
    }


LLM_RECIPES = {
    "missing_dependency": llm_missing_dependency,
    "upstream_code_edit": llm_upstream_edit,
    "same_line_conflict": llm_same_line_conflict,
    "mid_run_disconnect": llm_mid_run_disconnect,
    "runtime_exception": llm_runtime_exception,
    "corrupted_env_lock": llm_corrupted_lock,
    "stale_evidence_trap": llm_stale_evidence_trap,
    "duplicate_task_replay": llm_duplicate_replay,
}


# ---------------------------------------------------------------------------
# Case validation, evaluation, and reporting
# ---------------------------------------------------------------------------

FAULT_KINDS = frozenset(SCRIPTED_RECIPES)
CASE_REQUIRED_KEYS = frozenset(
    {
        "case_id",
        "title",
        "description",
        "initial_commit",
        "injected_fault",
        "user_prompt",
        "expected_tool_trace",
        "forbidden_hallucinations",
        "ground_truth_assertions",
    }
)


def validate_case(case: dict[str, Any], path: Path) -> None:
    """Reject case files that drift from the frozen `ChaosCase` contract."""
    missing = CASE_REQUIRED_KEYS - case.keys()
    if missing:
        raise RuntimeError(f"{path.name}: missing ChaosCase fields {sorted(missing)}")
    fault = case["injected_fault"]
    if set(fault) != {"kind", "target_path", "detail"}:
        raise RuntimeError(f"{path.name}: injected_fault must contain exactly kind/target_path/detail")
    if fault["kind"] not in FAULT_KINDS:
        raise RuntimeError(f"{path.name}: unknown injected_fault.kind {fault['kind']!r}")
    if not case["expected_tool_trace"]:
        raise RuntimeError(f"{path.name}: expected_tool_trace must not be empty")
    if not case["forbidden_hallucinations"]:
        raise RuntimeError(f"{path.name}: forbidden_hallucinations must not be empty")
    assertions = case["ground_truth_assertions"]
    if not assertions:
        raise RuntimeError(f"{path.name}: ground_truth_assertions must not be empty")
    ids = set()
    for assertion in assertions:
        if set(assertion) != {"assertion_id", "description", "metric_key", "expected_value"}:
            raise RuntimeError(
                f"{path.name}: assertion {assertion.get('assertion_id')!r} must contain exactly "
                "assertion_id/description/metric_key/expected_value"
            )
        if assertion["assertion_id"] in ids:
            raise RuntimeError(f"{path.name}: duplicate assertion_id {assertion['assertion_id']!r}")
        ids.add(assertion["assertion_id"])


def evaluate(case: dict[str, Any], metrics: dict[str, Any]) -> list[dict[str, Any]]:
    results = []
    for assertion in case["ground_truth_assertions"]:
        key = assertion["metric_key"]
        expected = assertion["expected_value"]
        actual = metrics.get(key)
        if isinstance(expected, bool):
            passed = isinstance(actual, bool) and actual == expected
        elif isinstance(expected, (int, float)) and not isinstance(expected, bool):
            passed = isinstance(actual, (int, float)) and not isinstance(actual, bool) and float(actual) == float(expected)
        else:
            passed = actual == expected
        results.append(
            {
                "assertion_id": assertion["assertion_id"],
                "metric_key": key,
                "expected": expected,
                "actual": actual,
                "passed": passed,
                "description": assertion["description"],
            }
        )
    return results


def run_scripted_case(case: dict[str, Any], daemon_bin: Path, workroot: Path, keep: bool) -> dict[str, Any]:
    kind = case["injected_fault"]["kind"]
    recipe = SCRIPTED_RECIPES[kind]

    started = time.perf_counter()
    case_dir = workroot / case["case_id"]
    if case_dir.exists():
        shutil.rmtree(case_dir)
    case_dir.mkdir(parents=True)
    repo = case_dir / "repo"
    data = case_dir / "data"
    init_repo(repo)

    metrics: dict[str, Any] = {}
    called_tools: list[str] = []
    notes: list[str] = []
    error: str | None = None
    try:
        outcome = recipe({"repo": repo, "data": data, "daemon_bin": daemon_bin, "bindir": case_dir})
        metrics = outcome["metrics"]
        called_tools = outcome["called_tools"]
    except Exception as exc:  # noqa: BLE001
        error = f"{type(exc).__name__}: {exc}"
        notes.append(error)
    elapsed = round((time.perf_counter() - started) * 1000.0, 1)

    expected_tools = list(case["expected_tool_trace"])
    tool_recall = compute_recall(expected_tools, called_tools)
    if tool_recall < 1.0:
        missing = [tool for tool in expected_tools if tool not in set(called_tools)]
        notes.append(f"expected_tool_trace missing: {missing}")
    assertions = evaluate(case, metrics)
    passed = error is None and tool_recall == 1.0 and all(item["passed"] for item in assertions)
    result = {
        "case_id": case["case_id"],
        "title": case["title"],
        "fault_kind": kind,
        "passed": passed,
        "duration_ms": elapsed,
        "expected_tool_trace": expected_tools,
        "called_tools": called_tools,
        "tool_recall": tool_recall,
        "metrics": metrics,
        "assertions": assertions,
        "notes": notes,
        "repo": str(repo) if keep else None,
    }
    if not keep:
        shutil.rmtree(case_dir, ignore_errors=True)
    return result


def run_llm_case(
    case: dict[str, Any], daemon_bin: Path, workroot: Path, keep: bool, llm_cfg: dict[str, Any]
) -> dict[str, Any]:
    kind = case["injected_fault"]["kind"]
    recipe = LLM_RECIPES[kind]

    started = time.perf_counter()
    case_dir = workroot / case["case_id"]
    if case_dir.exists():
        shutil.rmtree(case_dir)
    case_dir.mkdir(parents=True)
    repo = case_dir / "repo"
    data = case_dir / "data"
    init_repo(repo)

    metrics: dict[str, Any] = {}
    called_tools: list[str] = []
    expected_tools = list(case["expected_tool_trace"])
    agent_run: dict[str, Any] = {
        "local_tools": [],
        "turns": 0,
        "retries": 0,
        "loop_aborted": False,
        "prompt_tokens": 0,
        "completion_tokens": 0,
        "total_tokens": 0,
        "latency_ms": 0.0,
        "final_answer": "",
    }
    stale_hallucination = False
    notes: list[str] = []
    error: str | None = None
    try:
        outcome = recipe({"repo": repo, "data": data, "daemon_bin": daemon_bin, "bindir": case_dir}, case, llm_cfg)
        metrics = outcome["metrics"]
        called_tools = outcome["called_tools"]
        expected_tools = outcome["expected_llm_tools"]
        agent_run = outcome["agent_run"]
        stale_hallucination = bool(outcome["stale_hallucination"])
    except Exception as exc:  # noqa: BLE001
        error = f"{type(exc).__name__}: {exc}"
        notes.append(error)
    elapsed = round((time.perf_counter() - started) * 1000.0, 1)

    tool_recall = compute_recall(expected_tools, called_tools)
    if agent_run["loop_aborted"]:
        notes.append(f"LLM loop aborted: {agent_run['final_answer']}")
    if stale_hallucination:
        notes.append("LLM answer failed STALE anti-hallucination check")

    assertions = evaluate(case, metrics)
    assertions_ok = all(item["passed"] for item in assertions)
    passed = error is None and assertions_ok and not agent_run["loop_aborted"] and not stale_hallucination
    self_heal_passed: bool | None = None
    if kind in HEALING_FAULT_KINDS:
        self_heal_passed = bool(metrics.get("proposal_smoke_verified")) or (
            kind == "same_line_conflict" and metrics.get("task_state_final") == "completed"
        )

    result = {
        "case_id": case["case_id"],
        "title": case["title"],
        "fault_kind": kind,
        "passed": passed,
        "self_heal_passed": self_heal_passed,
        "duration_ms": elapsed,
        "expected_tool_trace": expected_tools,
        "called_tools": called_tools,
        "local_tools": agent_run["local_tools"],
        "tool_recall": tool_recall,
        "turns": agent_run["turns"],
        "retries": agent_run["retries"],
        "loop_aborted": agent_run["loop_aborted"],
        "stale_hallucination": stale_hallucination,
        "prompt_tokens": agent_run["prompt_tokens"],
        "completion_tokens": agent_run["completion_tokens"],
        "total_tokens": agent_run["total_tokens"],
        "latency_ms": agent_run["latency_ms"],
        "final_answer": agent_run["final_answer"],
        "metrics": metrics,
        "assertions": assertions,
        "notes": notes,
        "repo": str(repo) if keep else None,
    }
    if not keep:
        shutil.rmtree(case_dir, ignore_errors=True)
    return result


def find_daemon(explicit: str | None) -> Path:
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


def display_path(path: Path) -> str:
    """Report paths relative to the repo so committed evidence stays machine-agnostic."""
    try:
        return str(path.resolve().relative_to(REPO_ROOT))
    except ValueError:
        return path.name


HOME_PATH = re.compile(r"""(?:\b[A-Za-z]:[\\/]+Users[\\/]+|/(?:Users|home)/)[^\\/"'`<>\r\n]+(?=[\\/])""", re.IGNORECASE)


def redact_report_paths(value: Any) -> Any:
    if isinstance(value, str):
        return HOME_PATH.sub("~", value)
    if isinstance(value, list):
        return [redact_report_paths(item) for item in value]
    if isinstance(value, dict):
        return {key: redact_report_paths(item) for key, item in value.items()}
    return value


def write_scripted_reports(
    results: list[dict[str, Any]],
    p2p_gate: dict[str, Any] | None,
    out_dir: Path,
    daemon_bin: Path,
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    results = redact_report_paths(results)
    p2p_gate = redact_report_paths(p2p_gate)
    passed = sum(1 for result in results if result["passed"])
    covered = sorted({tool for result in results for tool in result["called_tools"]} & FROZEN_MCP_TOOLS)
    payload = {
        "suite": "relay-track4-chaos",
        "mode": "scripted",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "daemon_bin": display_path(daemon_bin),
        "python": sys.version.split()[0],
        "p2p_loopback_gate": p2p_gate,
        "mcp_tools_covered": covered,
        "summary": {
            "total": len(results),
            "passed": passed,
            "failed": len(results) - passed,
            "mcp_tools_covered_count": len(covered),
            "mcp_tools_total": len(FROZEN_MCP_TOOLS),
            "p2p_gate_passed": p2p_gate["passed"] if p2p_gate is not None else None,
        },
        "cases": results,
    }
    (out_dir / "chaos-report.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    lines = [
        "# Relay Chaos Suite Report (Track 4 — Scripted)",
        "",
        f"- Generated: {payload['generated_at']}",
        f"- Cases: {len(results)} · Passed: {passed} · Failed: {len(results) - passed}",
        f"- MCP Tools Covered: {len(covered)}/{len(FROZEN_MCP_TOOLS)} ({', '.join(covered)})",
    ]
    if p2p_gate is not None:
        gate_status = "PASS" if p2p_gate["passed"] else "FAIL"
        rtt_text = f"{p2p_gate['rtt_ms']} ms" if p2p_gate.get("rtt_ms") is not None else "n/a"
        lines.append(
            f"- P2P Loopback Gate (`sandbox-dual`): **{gate_status}** (RTT={rtt_text}, duration={p2p_gate['duration_ms']} ms)"
        )
    lines += [
        "",
        "| Case | Fault | Result | Tool Recall | Duration (ms) |",
        "|---|---|---|---:|---:|",
    ]
    for result in results:
        status = "PASS" if result["passed"] else "FAIL"
        lines.append(
            f"| `{result['case_id']}` | `{result['fault_kind']}` | {status} | {result['tool_recall']:.2f} | {result['duration_ms']} |"
        )
    for result in results:
        lines += ["", f"## {result['case_id']} — {result['title']}", ""]
        if result["notes"]:
            lines += ["Notes:", *(f"- {note}" for note in result["notes"]), ""]
        lines += ["| Assertion | Metric | Expected | Actual | Result |", "|---|---|---|---|---|"]
        for item in result["assertions"]:
            actual = json.dumps(item["actual"], ensure_ascii=False)
            expected = json.dumps(item["expected"], ensure_ascii=False)
            mark = "PASS" if item["passed"] else "FAIL"
            lines.append(f"| {item['assertion_id']} | `{item['metric_key']}` | {expected} | {actual} | {mark} |")
    (out_dir / "chaos-report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_llm_reports(
    results: list[dict[str, Any]],
    out_dir: Path,
    daemon_bin: Path,
    llm_cfg: dict[str, Any],
    *,
    report_stem: str = "chaos-llm-report",
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    results = redact_report_paths(results)
    passed = sum(1 for r in results if r["passed"])
    healing_cases = [r for r in results if r["self_heal_passed"] is not None]
    healing_passed = sum(1 for r in healing_cases if r["self_heal_passed"])
    stale_hits = sum(1 for r in results if r["stale_hallucination"])
    covered = sorted({tool for r in results for tool in r["called_tools"]} & FROZEN_MCP_TOOLS)
    latencies = [r["latency_ms"] for r in results] or [0.0]
    display_model = llm_cfg.get("label") or llm_cfg["model"]

    payload: dict[str, Any] = {
        "suite": "relay-track4-chaos-llm",
        "mode": "llm",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "daemon_bin": display_path(daemon_bin),
        "python": sys.version.split()[0],
        "model": display_model,
        "base_url": llm_cfg["base_url"],
    }
    if display_model != llm_cfg["model"]:
        payload["endpoint_model"] = llm_cfg["model"]
    if llm_cfg.get("reasoning_effort"):
        payload["reasoning_effort"] = llm_cfg["reasoning_effort"]
    payload["mcp_tools_covered"] = covered
    payload["summary"] = {
        "total": len(results),
        "passed": passed,
        "failed": len(results) - passed,
        "pass_rate_pct": round(100.0 * passed / len(results), 1),
        "self_heal_passed": healing_passed,
        "self_heal_total": len(healing_cases),
        "self_heal_rate_pct": round(100.0 * healing_passed / len(healing_cases), 1) if healing_cases else 0.0,
        "avg_tool_recall": round(sum(r["tool_recall"] for r in results) / len(results), 3),
        "total_retries": sum(r["retries"] for r in results),
        "stale_hallucination_count": stale_hits,
        "total_tokens": sum(r["total_tokens"] for r in results),
        "median_llm_latency_ms": round(statistics.median(latencies), 1),
        "mcp_tools_covered_count": len(covered),
    }
    payload["cases"] = results
    (out_dir / f"{report_stem}.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    summary = payload["summary"]
    lines = [
        "# Relay Chaos Suite Report (Track 4 — Live LLM)",
        "",
        f"- Generated: {payload['generated_at']}",
        f"- Model: `{display_model}` (`{llm_cfg['base_url']}`)",
        f"- Pass Rate: **{summary['passed']}/{summary['total']} ({summary['pass_rate_pct']}%)** · Self-Heal: **{summary['self_heal_passed']}/{summary['self_heal_total']} ({summary['self_heal_rate_pct']}%)**",
        f"- Avg Tool Recall: **{summary['avg_tool_recall']:.3f}** · Retries: **{summary['total_retries']}** · Stale Hallucinations: **{summary['stale_hallucination_count']}** · Total Tokens: **{summary['total_tokens']}**",
        "",
        "| Case | Fault | Pass | Self-Heal | Tool Recall | Retries | Stale Halluc | Turns | Tokens | LLM Latency (ms) |",
        "|---|---|---|---|---:|---:|---|---:|---:|---:|",
    ]
    for r in results:
        status = "PASS" if r["passed"] else "FAIL"
        heal_col = "n/a" if r["self_heal_passed"] is None else ("YES" if r["self_heal_passed"] else "NO")
        lines.append(
            f"| `{r['case_id']}` | `{r['fault_kind']}` | {status} | {heal_col} | "
            f"{r['tool_recall']:.2f} | {r['retries']} | {r['stale_hallucination']} | "
            f"{r['turns']} | {r['total_tokens']} | {r['latency_ms']} |"
        )
    for r in results:
        lines += ["", f"## {r['case_id']} — {r['title']}", ""]
        lines.append(f"- Called MCP tools: `{r['called_tools']}` (local read helpers: `{r['local_tools']}`)")
        if r["notes"]:
            lines += ["", "Notes:", *(f"- {note}" for note in r["notes"])]
        lines += ["", "| Assertion | Metric | Expected | Actual | Result |", "|---|---|---|---|---|"]
        for item in r["assertions"]:
            actual = json.dumps(item["actual"], ensure_ascii=False)
            expected = json.dumps(item["expected"], ensure_ascii=False)
            mark = "PASS" if item["passed"] else "FAIL"
            lines.append(f"| {item['assertion_id']} | `{item['metric_key']}` | {expected} | {actual} | {mark} |")
    (out_dir / f"{report_stem}.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def load_cases(selected: list[str] | None) -> list[dict[str, Any]]:
    all_cases: list[dict[str, Any]] = []
    seen_ids: set[str] = set()
    covered_kinds: set[str] = set()
    for path in sorted(CASES_DIR.glob("case_*.json")):
        case = json.loads(path.read_text(encoding="utf-8"))
        validate_case(case, path)
        if case["case_id"] in seen_ids:
            raise RuntimeError(f"duplicate case_id {case['case_id']!r}")
        seen_ids.add(case["case_id"])
        covered_kinds.add(case["injected_fault"]["kind"])
        all_cases.append(case)
    missing = set(SCRIPTED_RECIPES) - covered_kinds
    if missing:
        raise RuntimeError(f"no case JSON covers fault kinds: {sorted(missing)}")
    cases = [c for c in all_cases if not selected or c["case_id"] in selected]
    if not cases:
        raise RuntimeError("no matching chaos cases selected")
    return cases


def main() -> int:
    load_dotenv(REPO_ROOT / ".env")
    parser = argparse.ArgumentParser(description="Relay Track 4 dual-mode Chaos Suite judge")
    parser.add_argument("--mode", choices=("scripted", "llm"), default="scripted", help="Evaluation mode")
    parser.add_argument("--daemon-bin", default=None, help="Path to relay-daemon (defaults to target/{release,debug})")
    parser.add_argument("--case", action="append", help="Only run the given case_id (repeatable)")
    parser.add_argument("--out", default=str(RESULTS_DIR), help="Report output directory")
    parser.add_argument("--keep", action="store_true", help="Keep per-case repos and data directories")
    parser.add_argument("--skip-p2p-gate", action="store_true", help="Skip `relay-daemon sandbox-dual` loopback gate")
    parser.add_argument("--base-url", default=os.getenv("RELAY_LLM_BASE_URL", "http://127.0.0.1:11434/v1"))
    parser.add_argument("--model", default=os.getenv("RELAY_LLM_MODEL", "qwen3.5:4b"))
    parser.add_argument(
        "--label",
        default=None,
        help="Optional display model name recorded in LLM reports (defaults to --model)",
    )
    parser.add_argument(
        "--api-key-env",
        default=None,
        help="Env var name holding API key (omitted automatically for loopback URLs unless set explicitly)",
    )
    parser.add_argument(
        "--reasoning-effort",
        choices=("none", "low", "medium", "high"),
        default=None,
        help="Optional OpenAI reasoning_effort parameter forwarded to the endpoint",
    )
    parser.add_argument("--timeout", type=int, default=240, help="Per-request HTTP timeout in seconds")
    parser.add_argument(
        "--warmup",
        action="store_true",
        help="Send one unmeasured 1-token warmup request before running --mode llm cases",
    )
    parser.add_argument(
        "--report-stem",
        default="chaos-llm-report",
        help="Output filename stem (without extension) for --mode llm JSON/Markdown reports",
    )
    parser.add_argument("--max-turns", type=int, default=14)
    args = parser.parse_args()

    daemon_bin = find_daemon(args.daemon_bin)
    cases = load_cases(args.case)
    workroot = (REPO_ROOT / ".tmp" / "chaos-work") if args.keep else Path(tempfile.mkdtemp(prefix="relay-chaos-"))
    workroot.mkdir(parents=True, exist_ok=True)
    out_dir = Path(args.out)

    try:
        if args.mode == "scripted":
            print(f"Relay Chaos Suite [scripted] · {len(cases)} cases · daemon={display_path(daemon_bin)}", flush=True)
            p2p_gate: dict[str, Any] | None = None
            if not args.skip_p2p_gate:
                p2p_gate = run_p2p_loopback_gate(daemon_bin, workroot, keep=args.keep)
                gate_mark = "PASS" if p2p_gate["passed"] else "FAIL"
                rtt_str = f"{p2p_gate['rtt_ms']}ms" if p2p_gate["rtt_ms"] is not None else "n/a"
                print(
                    f"[P2P] {gate_mark} sandbox-dual (rtt={rtt_str}, {p2p_gate['duration_ms']} ms)",
                    flush=True,
                )

            results = []
            for index, case in enumerate(cases, start=1):
                result = run_scripted_case(case, daemon_bin, workroot, args.keep)
                results.append(result)
                status = "PASS" if result["passed"] else "FAIL"
                print(
                    f"[{index}/{len(cases)}] {status} {result['case_id']} "
                    f"(recall={result['tool_recall']:.2f}, {result['duration_ms']} ms)",
                    flush=True,
                )
                if not result["passed"]:
                    for item in result["assertions"]:
                        if not item["passed"]:
                            print(f"    {item['assertion_id']}: expected={item['expected']!r} actual={item['actual']!r}", flush=True)
                    for note in result["notes"]:
                        print(f"    note: {note}", flush=True)

            write_scripted_reports(results, p2p_gate, out_dir, daemon_bin)
            failed = [r for r in results if not r["passed"]]
            gate_ok = True if p2p_gate is None else p2p_gate["passed"]
            print(
                f"\nSummary: {len(results) - len(failed)}/{len(results)} passed · "
                f"report: {out_dir / 'chaos-report.md'}",
                flush=True,
            )
            return 0 if (not failed and gate_ok) else 1

        if args.api_key_env is not None:
            api_key = os.getenv(args.api_key_env) if args.api_key_env else None
        elif is_loopback_url(args.base_url):
            api_key = None
        else:
            default_env = os.getenv("RELAY_LLM_API_KEY_ENV", "RELAY_LLM_API_KEY")
            api_key = os.getenv(default_env) if default_env else None
        llm_cfg = {
            "base_url": args.base_url,
            "model": args.model,
            "label": args.label,
            "api_key": api_key,
            "reasoning_effort": args.reasoning_effort,
            "timeout_secs": args.timeout,
            "max_turns": args.max_turns,
        }
        if args.warmup:
            warmup_llm(llm_cfg)
        display_model = args.label or args.model
        print(
            f"Relay Chaos Suite [llm] · model={display_model} · {len(cases)} cases · daemon={display_path(daemon_bin)}",
            flush=True,
        )
        results = []
        for index, case in enumerate(cases, start=1):
            result = run_llm_case(case, daemon_bin, workroot, args.keep, llm_cfg)
            results.append(result)
            status = "PASS" if result["passed"] else "FAIL"
            print(
                f"[{index}/{len(cases)}] {status} {result['case_id']} "
                f"(recall={result['tool_recall']:.2f}, retries={result['retries']}, "
                f"tokens={result['total_tokens']}, {result['duration_ms']} ms)",
                flush=True,
            )
            if not result["passed"]:
                for item in result["assertions"]:
                    if not item["passed"]:
                        print(f"    {item['assertion_id']}: expected={item['expected']!r} actual={item['actual']!r}", flush=True)
                for note in result["notes"]:
                    print(f"    note: {note}", flush=True)

        write_llm_reports(results, out_dir, daemon_bin, llm_cfg, report_stem=args.report_stem)
        failed = [r for r in results if not r["passed"]]
        print(
            f"\nSummary: {len(results) - len(failed)}/{len(results)} passed · "
            f"report: {out_dir / f'{args.report_stem}.md'}",
            flush=True,
        )
        return 0 if not failed else 1
    finally:
        if not args.keep:
            shutil.rmtree(workroot, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
