#!/usr/bin/env python3
"""Relay Track 4 — one-click Chaos Suite judge.

Drives the real `relay-daemon mcp-stdio` server against an isolated copy of
`bench/fixture-repo` for each frozen `ChaosCase` fault kind, then evaluates
every `ground_truth_assertions` entry against metrics observed from the running
system (MCP tool output, the SQLite task store, Git state, and
`relay-daemon compose-context`).

Design notes:
* No LLM and no PyPI access: repairs are deterministic scripted steps, and
  dependency healing installs the vendored `tiny_stats` wheel offline.
* The JSON cases stay declarative (`ChaosCase` contract). Execution recipes are
  keyed by `injected_fault.kind` so the frozen schema is not extended.

Usage:
    uv run python bench/chaos-cases/run_chaos_suite.py
    uv run python bench/chaos-cases/run_chaos_suite.py --case chaos-04-mid-run-disconnect --keep
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import queue
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_SRC = REPO_ROOT / "bench" / "fixture-repo"
CASES_DIR = Path(__file__).resolve().parent
RESULTS_DIR = CASES_DIR / "results"

VENDOR_WHEEL = "./vendor/tiny_stats-0.1.0-py3-none-any.whl"
VALID_LOCKFILE = (
    'version = 1\nrevision = 3\nrequires-python = ">=3.11"\n\n'
    '[[package]]\nname = "relay-forecast-fixture"\nversion = "0.1.0"\n'
    'source = { virtual = "." }\n'
)
CORRUPTED_LOCKFILE = "version = 1\nrevision = 3\n\n[[package]\nname = \n"


# ---------------------------------------------------------------------------
# Environment & process helpers
# ---------------------------------------------------------------------------


def fixture_env() -> dict[str, str]:
    """Inherit the shell environment but force uv into deterministic offline mode."""
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
            "RUST_LOG": "warn",
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


def is_dirty(repo: Path) -> bool:
    return git(repo, "status", "--porcelain").stdout.strip() != ""


def copy_fixture(dest: Path) -> None:
    shutil.copytree(
        FIXTURE_SRC,
        dest,
        ignore=shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc", ".DS_Store"),
    )


def init_repo(dest: Path) -> None:
    copy_fixture(dest)
    (dest / ".gitignore").write_text(".venv/\n.relay/\n__pycache__/\n", encoding="utf-8", newline="\n")
    run_cmd(["git", "init", "--initial-branch=main"], cwd=dest)
    git(dest, "config", "core.autocrlf", "false")
    git(dest, "config", "user.name", "Relay Chaos Suite")
    git(dest, "config", "user.email", "chaos@relay.local")
    commit_all(dest, "chore(bench): initialize forecast fixture")


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
# Minimal MCP stdio client
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
            env=fixture_env(),
        )
        self._next_id = 1
        self._queue: queue.Queue[str] = queue.Queue()
        self._stderr: list[str] = []
        self._killed = False
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
        self.request("tools/list", {})

    def call(self, name: str, arguments: dict[str, Any] | None = None, timeout: float = 240.0) -> tuple[bool, Any, str]:
        result = self.request("tools/call", {"name": name, "arguments": arguments or {}}, timeout=timeout)
        is_error = bool(result.get("isError", False))
        text = "\n".join(block.get("text", "") for block in result.get("content", []) if block.get("type") == "text")
        payload: Any = None
        try:
            payload = json.loads(text)
        except json.JSONDecodeError:
            payload = None
        return is_error, payload, text

    def kill(self) -> None:
        self._killed = True
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
                self.proc.kill()
                self.proc.wait(timeout=5)

    def __enter__(self) -> "Daemon":
        self.initialize()
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()


# ---------------------------------------------------------------------------
# CLI entry points shared by recipes
# ---------------------------------------------------------------------------


def submit_task(daemon_bin: Path, repo: Path, data: Path, preset: str, params: list[str] | None = None) -> str:
    cmd = [str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "submit-task", "--preset", preset]
    for param in params or []:
        cmd.extend(["--param", param])
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


# ---------------------------------------------------------------------------
# Metric extraction
# ---------------------------------------------------------------------------


def dag_snapshot(daemon: Daemon) -> dict[str, Any]:
    ok, payload, text = daemon.call("get_dag_snapshot", {"include_stale": True})
    if ok or payload is None:
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


# ---------------------------------------------------------------------------
# Case recipes — one per frozen InjectedFaultKind
# ---------------------------------------------------------------------------


def case_missing_dependency(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin, bindir = ctx["repo"], ctx["data"], ctx["daemon_bin"], ctx["bindir"]
    forecast = repo / "src" / "forecast.py"
    write_text(forecast, read_text(forecast) + "\nimport tiny_stats  # noqa: F401\n")
    commit_all(repo, "feat(bench): probe an undeclared vendored dependency")

    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        ok, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if ok or report is None:
            raise RuntimeError(f"preflight should fail with a missing module: {text}")
        metrics["detected_missing_module"] = report.get("detected_missing_module")

        ok, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "uv_spec",
                "summary": "Declare tiny_stats from the offline vendor wheel",
                "content": VENDOR_WHEEL,
            },
        )
        if ok or proposal is None:
            raise RuntimeError(f"uv_spec proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]
        metrics["approval_required_observed"] = read_task(data, task_id)["state"] == "self_healing_proposed"

        ok, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if ok or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon)
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
    return metrics


def case_upstream_edit(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        ok, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if ok or response is None:
            raise RuntimeError(f"baseline execution failed: {text}")

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
            ok, _, _ = daemon.call("read_evidence_log", {"node_id": stale_id})
            metrics["stale_read_blocked"] = ok
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
    if metrics["stale_evidence_count"] != 1:
        raise RuntimeError(
            f"expected exactly one stale evidence node, saw {metrics['stale_evidence_count']}"
        )
    return metrics


def case_same_line_conflict(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin, bindir = ctx["repo"], ctx["data"], ctx["daemon_bin"], ctx["bindir"]
    client = bindir / "laptop-checkout"
    shutil.copytree(repo, client, ignore=shutil.ignore_patterns(".venv", "__pycache__"))
    client_forecast = client / "src" / "forecast.py"
    write_text(
        client_forecast,
        read_text(client_forecast).replace("default=0.35", "default=0.55", 1),
    )

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
        ok, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Raise the default EWMA alpha to 0.45",
                "content": patch,
            },
        )
        if ok or proposal is None:
            raise RuntimeError(f"code_diff proposal failed: {text}")
        ok, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if ok or response is None:
            raise RuntimeError(f"host execution failed: {text}")
        snapshot = dag_snapshot(daemon)
        diff_preview = proposal["diff_preview"]

    patch_file = bindir / "host-patch.diff"
    patch_file.write_text(diff_preview, encoding="utf-8", newline="\n")
    apply_check = git(client, "apply", "--check", "--whitespace=nowarn", str(patch_file), check=False)
    metrics["conflict_apply_failed"] = apply_check.returncode != 0
    metrics["client_edit_preserved"] = "default=0.55" in read_text(client_forecast)
    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_fresh_count": stats["fresh"],
            "stale_evidence_count": stats["stale"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    if not metrics["conflict_apply_failed"]:
        raise RuntimeError("expected the host diff to conflict with the laptop edit")
    return metrics


def case_mid_run_disconnect(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
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
    task_id = submit_task(daemon_bin, repo, data, "train_grid")

    first = Daemon(daemon_bin, repo, data)
    first.initialize()
    failure: list[str] = []

    def preflight() -> None:
        try:
            first.call("run_preflight_in_worktree", {"task_id": task_id}, timeout=120)
        except McpError as error:
            failure.append(str(error))

    worker = threading.Thread(target=preflight, daemon=True)
    worker.start()
    # Kill only once preflight is actually running, so the crash has persisted state to recover.
    deadline = time.monotonic() + 4
    while time.monotonic() < deadline:
        if read_task(data, task_id)["state"] == "preflight_running":
            break
        time.sleep(0.1)
    first.kill()
    worker.join(timeout=10)
    # Let any orphaned `uv run` child finish before reopening the same worktree.

    time.sleep(8)
    metrics: dict[str, Any] = {"crash_state_persisted": read_task(data, task_id)["state"]}
    with Daemon(daemon_bin, repo, data) as daemon:
        ok, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id}, timeout=240)
        if ok or report is None:
            raise RuntimeError(f"resumed preflight failed: {text}\n--\n{daemon.stderr_tail()}")
        metrics["resume_preflight_passed"] = bool(report["smoke_passed"])
        ok, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if ok or response is None:
            raise RuntimeError(f"post-reconnect execution failed: {text}")
        snapshot = dag_snapshot(daemon)
    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_count": stats["total"],
            "evidence_fresh_count": stats["fresh"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return metrics


def case_runtime_exception(ctx: dict[str, Any]) -> dict[str, Any]:
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
        ok, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if ok or report is None:
            raise RuntimeError(f"preflight should fail with a NameError: {text}")
        metrics["preflight_error"] = (report.get("stderr_tail") or "").strip().splitlines()[-1:]
        ok, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Define DEFAULT_ALPHA = 0.35 before the argparse default is evaluated",
                "content": patch,
            },
        )
        if ok or proposal is None:
            raise RuntimeError(f"code_diff proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]
        ok, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if ok or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon)
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
    return metrics


def case_corrupted_lock(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    write_text(repo / "uv.lock", CORRUPTED_LOCKFILE)
    commit_all(repo, "fix(bench): merge a truncated uv.lock")
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    patch = unified_diff(CORRUPTED_LOCKFILE, VALID_LOCKFILE, "uv.lock")

    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        ok, report, text = daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        if ok or report is None:
            raise RuntimeError(f"preflight should fail on the corrupted lockfile: {text}")
        stderr = (report.get("stderr_tail") or "") + (report.get("raw_log") or "")
        metrics["uv_lock_parse_failed"] = "Failed to parse `uv.lock`" in stderr
        ok, proposal, text = daemon.call(
            "propose_worktree_patch",
            {
                "task_id": task_id,
                "patch_kind": "code_diff",
                "summary": "Restore a valid uv.lock so the offline environment can start",
                "content": patch,
            },
        )
        if ok or proposal is None:
            raise RuntimeError(f"lockfile repair proposal failed: {text}")
        metrics["proposal_smoke_verified"] = proposal["smoke_verified"]
        ok, response, text = daemon.call(
            "approve_and_execute_task",
            {"task_id": task_id, "patch_id": proposal["patch_id"], "approved_via_ui": True},
        )
        if ok or response is None:
            raise RuntimeError(f"approved execution failed: {text}")
        snapshot = dag_snapshot(daemon)
    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "task_state_final": read_task(data, task_id)["state"],
            "evidence_fresh_count": stats["fresh"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    return metrics


def case_stale_evidence_trap(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        ok, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if ok or response is None:
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
        ok, _, _ = daemon.call("read_evidence_log", {"node_id": stale_id})
        metrics["stale_read_blocked"] = ok

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
    return metrics


def case_duplicate_replay(ctx: dict[str, Any]) -> dict[str, Any]:
    repo, data, daemon_bin = ctx["repo"], ctx["data"], ctx["daemon_bin"]
    task_id = submit_task(daemon_bin, repo, data, "train_grid")
    metrics: dict[str, Any] = {}
    with Daemon(daemon_bin, repo, data) as daemon:
        daemon.call("run_preflight_in_worktree", {"task_id": task_id})
        ok, response, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if ok or response is None:
            raise RuntimeError(f"initial execution failed: {text}")
        first_evidence = read_task(data, task_id)["evidence_node_id"]

        ok, replay, text = daemon.call(
            "approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True}
        )
        if ok or replay is None:
            raise RuntimeError(f"replayed approval failed: {text}")
        metrics["replay_response"] = replay.get("outcome")
        snapshot = dag_snapshot(daemon)

    stats = evidence_stats(snapshot)
    metrics.update(
        {
            "replay_evidence_count": stats["total"],
            "evidence_node_id_stable": read_task(data, task_id)["evidence_node_id"] == first_evidence,
            "task_state_final": read_task(data, task_id)["state"],
            "main_worktree_dirty": is_dirty(repo),
        }
    )
    if not metrics["evidence_node_id_stable"]:
        raise RuntimeError("replay replaced the evidence node")
    return metrics


RECIPES = {
    "missing_dependency": case_missing_dependency,
    "upstream_code_edit": case_upstream_edit,
    "same_line_conflict": case_same_line_conflict,
    "mid_run_disconnect": case_mid_run_disconnect,
    "runtime_exception": case_runtime_exception,
    "corrupted_env_lock": case_corrupted_lock,
    "stale_evidence_trap": case_stale_evidence_trap,
    "duplicate_task_replay": case_duplicate_replay,
}

FAULT_KINDS = frozenset(RECIPES)
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



# ---------------------------------------------------------------------------
# Assertion evaluation & reporting
# ---------------------------------------------------------------------------


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


def run_case(case: dict[str, Any], daemon_bin: Path, workroot: Path, keep: bool) -> dict[str, Any]:
    kind = case["injected_fault"]["kind"]
    recipe = RECIPES.get(kind)
    if recipe is None:
        raise RuntimeError(f"no execution recipe for fault kind '{kind}'")

    started = time.perf_counter()
    case_dir = workroot / case["case_id"]
    if case_dir.exists():
        shutil.rmtree(case_dir)
    case_dir.mkdir(parents=True)
    repo = case_dir / "repo"
    data = case_dir / "data"
    init_repo(repo)

    metrics: dict[str, Any] = {}
    notes: list[str] = []
    error: str | None = None
    try:
        metrics = recipe({"repo": repo, "data": data, "daemon_bin": daemon_bin, "bindir": case_dir})
    except Exception as exc:  # noqa: BLE001 - the judge must report every failure
        error = f"{type(exc).__name__}: {exc}"
        notes.append(error)
    elapsed = round((time.perf_counter() - started) * 1000.0, 1)

    assertions = evaluate(case, metrics)
    passed = error is None and all(item["passed"] for item in assertions)
    result = {
        "case_id": case["case_id"],
        "title": case["title"],
        "fault_kind": kind,
        "passed": passed,
        "duration_ms": elapsed,
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


def write_reports(results: list[dict[str, Any]], out_dir: Path, daemon_bin: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    passed = sum(1 for result in results if result["passed"])
    payload = {
        "suite": "relay-track4-chaos",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "daemon_bin": display_path(daemon_bin),
        "python": sys.version.split()[0],
        "summary": {"total": len(results), "passed": passed, "failed": len(results) - passed},
        "cases": results,
    }
    (out_dir / "chaos-report.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    lines = [
        "# Relay Chaos Suite Report (Track 4)",
        "",
        f"- Generated: {payload['generated_at']}",
        f"- Cases: {len(results)} · Passed: {passed} · Failed: {len(results) - passed}",
        "",
        "| Case | Fault | Result | Duration (ms) |",
        "|---|---|---|---|",
    ]
    for result in results:
        status = "PASS" if result["passed"] else "FAIL"
        lines.append(f"| `{result['case_id']}` | `{result['fault_kind']}` | {status} | {result['duration_ms']} |")
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


def load_cases(selected: list[str] | None) -> list[dict[str, Any]]:
    cases = []
    seen_ids: set[str] = set()
    for path in sorted(CASES_DIR.glob("case_*.json")):
        case = json.loads(path.read_text(encoding="utf-8"))
        validate_case(case, path)
        if case["case_id"] in seen_ids:
            raise RuntimeError(f"duplicate case_id {case['case_id']!r}")
        seen_ids.add(case["case_id"])
        if selected and case["case_id"] not in selected:
            continue
        cases.append(case)
    missing = {kind for kind in RECIPES} - {
        json.loads(path.read_text(encoding="utf-8"))["injected_fault"]["kind"]
        for path in CASES_DIR.glob("case_*.json")
    }
    if missing:
        raise RuntimeError(f"no case JSON covers fault kinds: {sorted(missing)}")
    if not cases:
        raise RuntimeError("no matching chaos cases selected")
    return cases


def main() -> int:
    parser = argparse.ArgumentParser(description="Relay Track 4 Chaos Suite judge")
    parser.add_argument("--daemon-bin", default=None, help="Path to relay-daemon (defaults to target/{release,debug})")
    parser.add_argument("--case", action="append", help="Only run the given case_id (repeatable)")
    parser.add_argument("--out", default=str(RESULTS_DIR), help="Report output directory")
    parser.add_argument("--keep", action="store_true", help="Keep per-case repos and data directories")
    args = parser.parse_args()

    daemon_bin = find_daemon(args.daemon_bin)
    cases = load_cases(args.case)
    workroot = Path(tempfile.mkdtemp(prefix="relay-chaos-")) if not args.keep else (REPO_ROOT / ".tmp" / "chaos-work")
    workroot.mkdir(parents=True, exist_ok=True)

    print(f"Relay Chaos Suite · {len(cases)} cases · daemon={display_path(daemon_bin)}")
    results = []
    for index, case in enumerate(cases, start=1):
        result = run_case(case, daemon_bin, workroot, args.keep)
        results.append(result)
        status = "PASS" if result["passed"] else "FAIL"
        print(f"[{index}/{len(cases)}] {status} {result['case_id']} ({result['duration_ms']} ms)")
        if not result["passed"]:
            for item in result["assertions"]:
                if not item["passed"]:
                    print(f"    {item['assertion_id']}: expected={item['expected']!r} actual={item['actual']!r}")
            for note in result["notes"]:
                print(f"    note: {note}")

    write_reports(results, Path(args.out), daemon_bin)
    failed = [result for result in results if not result["passed"]]
    print(f"\nSummary: {len(results) - len(failed)}/{len(results)} passed · report: {Path(args.out) / 'chaos-report.md'}")
    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
