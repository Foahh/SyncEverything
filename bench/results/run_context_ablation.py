#!/usr/bin/env python3
"""Track 5 Experiment 2 — Deterministic Context Representation Ablation.

Evaluates Relay's three context composition strategies exposed by
`relay-daemon compose-context --all-strategies --no-sync`:

  1. `full_transcript` — Full chronological handoffs, tasks, and raw DAG nodes
  2. `flat_summary`    — Compact flat summary without causal quarantine
  3. `dag_bundle`      — Decision-Evidence DAG bundle with STALE quarantine

Part A builds five real progressive DAG snapshots on isolated checkouts of
`bench/fixture-repo` (1, 3, 6, 10 stale evidence rounds + a mixed 5-stale /
1-fresh snapshot) and verifies blake3 hash determinism, token scaling, and
`exposed_stale_metrics_count` directly from `relay-daemon compose-context`.

Part B evaluates live LLMs (default: deepseek-flash, qwen3.5:4b, qwen3.5:9b)
against the exact verbatim composed contexts across 6 stale-trap, mixed-
freshness, and decision-constraint queries.

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


REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_SRC = REPO_ROOT / "bench" / "fixture-repo"
DEFAULT_OUT = Path(__file__).resolve().parent
STRATEGIES = ("full_transcript", "flat_summary", "dag_bundle")
TEXT_SUFFIXES = {".py", ".toml", ".lock", ".md", ".txt", ".sh", ".json"}
IGNORE_PATTERNS = shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc", ".DS_Store")

STALE_ACK_KEYWORDS = (
    "stale",
    "outdated",
    "superseded",
    "invalid",
    "quarantined",
    "re-run",
    "rerun",
    "no fresh",
    "no valid",
    "cannot verify",
    "not valid",
    "过期",
    "陈旧",
    "失效",
    "重跑",
)

ABLATION_SYSTEM_PROMPT = (
    "You are a research experiment assistant. Answer using ONLY the supplied context bundle. "
    "Never report outdated/stale metrics as valid for the current HEAD commit. "
    "If current HEAD has no fresh verified metrics, explicitly say no valid/fresh result exists "
    "for HEAD and recommend re-running instead of quoting old numbers."
)


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
    run_cmd(["git", "config", "user.name", "Relay Ablation"], cwd=dest)
    run_cmd(["git", "config", "user.email", "ablation@relay.local"], cwd=dest)
    run_cmd(["git", "add", "--all"], cwd=dest)
    run_cmd(["git", "commit", "-m", "chore(bench): initial forecast baseline"], cwd=dest)
    return run_cmd(["git", "rev-parse", "HEAD"], cwd=dest).stdout.strip()


class McpStdioClient:
    """Minimal JSON-RPC 2.0 client driving `relay-daemon mcp-stdio`."""

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
                "clientInfo": {"name": "relay-track5-ablation", "version": "1.0.0"},
            },
        )
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
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
                raise RuntimeError("mcp-stdio exited prematurely:\n" + "".join(self._stderr_lines))
            stripped = line.strip()
            if not stripped:
                continue
            resp = json.loads(stripped)
            if resp.get("id") == req_id:
                if "error" in resp:
                    raise RuntimeError(f"MCP error {method}: {resp['error']}")
                return resp["result"]

    def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        res = self.request("tools/call", {"name": name, "arguments": arguments})
        is_err = bool(res.get("isError", False))
        blocks = res.get("content", [])
        text = "\n".join(b.get("text", "") for b in blocks if b.get("type") == "text")
        if is_err:
            raise RuntimeError(f"MCP tool {name} failed: {text}")
        return json.loads(text)


def update_blend_and_commit(repo: Path, w_ma: float, message: str) -> str:
    w_ewma = round(1.0 - w_ma, 2)
    forecast = repo / "src" / "forecast.py"
    text = forecast.read_text(encoding="utf-8")
    new_text, count = re.subn(
        r"pred = [0-9.]+ \* ma \+ [0-9.]+ \* ewma",
        f"pred = {w_ma:.2f} * ma + {w_ewma:.2f} * ewma",
        text,
        count=1,
    )
    if count != 1:
        raise RuntimeError("Could not locate forecast blend line in src/forecast.py")
    forecast.write_text(new_text, encoding="utf-8", newline="\n")
    run_cmd(["git", "add", "src/forecast.py"], cwd=repo)
    run_cmd(["git", "commit", "-m", message], cwd=repo)
    return run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()


def insert_decision_node(db_path: Path, round_idx: int, commit_sha: str, w_ma: float) -> None:
    node_id = f"decision:r{round_idx}"
    edge_id = f"informs:r{round_idx}"
    artifact_id = f"artifact:{commit_sha}"
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f000Z")
    payload = {
        "kind": "decision",
        "rationale": (
            f"Round {round_idx}: evaluate blend weight {w_ma:.2f}*MA + {1.0 - w_ma:.2f}*EWMA "
            "under fixed window grid."
        ),
        "constraints": [
            "Keep windows=7,14,28 across grid evaluations",
            "Run strictly offline with UV_OFFLINE=1 and clean main worktree",
        ],
    }
    freshness = {"status": "fresh"}
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
                f"Blend policy r{round_idx} (MA={w_ma:.2f})",
                json.dumps(payload),
                json.dumps(freshness),
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
            (edge_id, "forecast-bench", node_id, artifact_id, json.dumps("informs")),
        )
        conn.commit()
    finally:
        conn.close()


def checkpoint_sqlite(db_path: Path) -> None:
    conn = sqlite3.connect(str(db_path))
    try:
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        conn.commit()
    finally:
        conn.close()


def execute_round(daemon_bin: Path, repo: Path, data: Path, round_idx: int, w_ma: float) -> None:
    sha = update_blend_and_commit(
        repo,
        w_ma,
        f"feat(forecast): round {round_idx} blend {w_ma:.2f}/{1.0 - w_ma:.2f}",
    )
    run_cmd([str(daemon_bin), "--repo-root", str(repo), "--data-dir", str(data), "sync-head"])
    insert_decision_node(data / "relay.db", round_idx, sha, w_ma)
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
            f"Round {round_idx}: sweep windows=7,14,28 at MA={w_ma:.2f}, strictly offline UV_OFFLINE=1",
        ]
    ).stdout
    task_id = json.loads(submit_out)["spec"]["task_id"]
    with McpStdioClient(daemon_bin, repo, data) as mcp:
        pre = mcp.call("run_preflight_in_worktree", {"task_id": task_id})
        if not pre.get("smoke_passed", False):
            raise RuntimeError(f"Round {round_idx} smoke preflight failed: {pre}")
        mcp.call("approve_and_execute_task", {"task_id": task_id, "approved_via_ui": True})


def extract_metric_tokens(metrics_json: dict[str, Any]) -> tuple[list[str], str | None]:
    """Return all numeric tokens present in an Evidence node + best_mae token."""
    tokens: set[str] = set()
    best_mae_tok: str | None = None
    best_mae = metrics_json.get("best_mae")
    if isinstance(best_mae, (int, float)):
        best_mae_tok = f"{float(best_mae):.5f}"
        tokens.add(best_mae_tok)
        tokens.add(best_mae_tok[:6])
    for run in metrics_json.get("runs", []) or []:
        for key in ("mae", "rmse"):
            val = run.get(key)
            if isinstance(val, (int, float)):
                s = f"{float(val):.5f}"
                tokens.add(s)
                tokens.add(s[:6])
    return sorted(tokens), best_mae_tok


def read_snapshot_metrics(daemon_bin: Path, repo: Path, data: Path) -> dict[str, Any]:
    _ = daemon_bin
    db_path = data / "relay.db"
    checkpoint_sqlite(db_path)

    stale_tokens: set[str] = set()
    fresh_tokens: set[str] = set()
    fresh_best_maes: list[str] = []
    stale_best_maes: list[str] = []

    conn = sqlite3.connect(str(db_path))
    try:
        rows = conn.execute(
            "SELECT payload_json, freshness_json FROM dag_nodes ORDER BY created_at ASC, rowid ASC"
        ).fetchall()
    finally:
        conn.close()

    for payload_raw, freshness_raw in rows:
        payload = json.loads(payload_raw)
        if payload.get("kind") != "evidence":
            continue
        freshness = json.loads(freshness_raw)
        tokens, best_tok = extract_metric_tokens(payload.get("metrics_json") or {})
        status = freshness.get("status")
        if status == "stale":
            stale_tokens.update(tokens)
            if best_tok:
                stale_best_maes.append(best_tok)
        else:
            fresh_tokens.update(tokens)
            if best_tok:
                fresh_best_maes.append(best_tok)

    head_commit_sha = run_cmd(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()
    # Exclude any prefix/number shared with a truly fresh HEAD metric
    exclusive_stale = sorted(tok for tok in stale_tokens if tok not in fresh_tokens)
    return {
        "head_commit_sha": head_commit_sha,
        "stale_best_maes": stale_best_maes,
        "fresh_best_maes": fresh_best_maes,
        "stale_tokens": exclusive_stale,
    }


def compose_twice_and_verify(daemon_bin: Path, repo: Path, data: Path) -> dict[str, Any]:
    raw1 = run_cmd(
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
    ).stdout
    raw2 = run_cmd(
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
    ).stdout
    first = json.loads(raw1)
    second = json.loads(raw2)
    for strat in STRATEGIES:
        h1 = first[strat]["context_bundle_hash"]
        h2 = second[strat]["context_bundle_hash"]
        if h1 != h2:
            raise RuntimeError(f"blake3 hash mismatch for {strat}: {h1} != {h2}")
        if first[strat]["content"] != second[strat]["content"]:
            raise RuntimeError(f"content mismatch across repeated compose-context for {strat}")
    if first["dag_bundle"]["exposed_stale_metrics_count"] != 0:
        raise RuntimeError(
            f"Expected dag_bundle exposed_stale_metrics_count=0, got {first['dag_bundle']['exposed_stale_metrics_count']}"
        )
    return first


def clone_checkpoint(src_repo: Path, src_data: Path, dst_dir: Path) -> tuple[Path, Path]:
    checkpoint_sqlite(src_data / "relay.db")
    dst_repo = dst_dir / "repo"
    dst_data = dst_dir / "data"
    shutil.copytree(src_repo, dst_repo, ignore=shutil.ignore_patterns(".venv", "__pycache__"))
    shutil.copytree(src_data, dst_data, ignore=shutil.ignore_patterns("worktrees", "blobs"))
    return dst_repo, dst_data


def build_all_snapshots(daemon_bin: Path, workroot: Path) -> dict[str, dict[str, Any]]:
    base_repo = workroot / "base" / "repo"
    base_data = workroot / "base" / "data"
    base_repo.parent.mkdir(parents=True, exist_ok=True)
    init_fixture_repo(base_repo)

    weights = [0.50 - 0.03 * i for i in range(1, 11)]  # 0.47 down to 0.20
    checkpoints = {1: "depth_1", 3: "depth_3", 6: "depth_6", 10: "depth_10"}
    snapshots: dict[str, dict[str, Any]] = {}

    for idx, w_ma in enumerate(weights, start=1):
        execute_round(daemon_bin, base_repo, base_data, idx, w_ma)

        if idx == 5:
            # Build depth_mixed_6 from round 5 + 1 new commit + 1 fresh run on HEAD
            m_repo, m_data = clone_checkpoint(base_repo, base_data, workroot / "depth_mixed_6")
            execute_round(daemon_bin, m_repo, m_data, 6, 0.32)
            bundles = compose_twice_and_verify(daemon_bin, m_repo, m_data)
            meta = read_snapshot_metrics(daemon_bin, m_repo, m_data)
            snapshots["depth_mixed_6"] = {
                "snapshot_id": "depth_mixed_6",
                "history_rounds": 6,
                "bundles": bundles,
                **meta,
            }

        if idx in checkpoints:
            snap_id = checkpoints[idx]
            s_repo, s_data = clone_checkpoint(base_repo, base_data, workroot / snap_id)
            update_blend_and_commit(s_repo, 0.14, f"feat(forecast): supersede history after {idx} rounds")
            sync_out = json.loads(
                run_cmd([str(daemon_bin), "--repo-root", str(s_repo), "--data-dir", str(s_data), "sync-head"]).stdout
            )
            if sync_out.get("status") != "superseded":
                raise RuntimeError(f"Expected superseded on {snap_id}, got {sync_out}")
            bundles = compose_twice_and_verify(daemon_bin, s_repo, s_data)
            meta = read_snapshot_metrics(daemon_bin, s_repo, s_data)
            snapshots[snap_id] = {
                "snapshot_id": snap_id,
                "history_rounds": idx,
                "bundles": bundles,
                **meta,
            }

    return snapshots


def build_queries(snapshots: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {
            "query_id": "Q1_depth1_stale_trap",
            "snapshot_id": "depth_1",
            "kind": "stale_trap",
            "prompt": (
                f"Current HEAD commit is {snapshots['depth_1']['head_commit_sha'][:7]}. "
                "What is the verified best MAE and best window for the current HEAD commit? "
                "Only cite metrics that are verified and fresh for current HEAD."
            ),
        },
        {
            "query_id": "Q2_depth3_stale_trap",
            "snapshot_id": "depth_3",
            "kind": "stale_trap",
            "prompt": (
                f"Current HEAD commit is {snapshots['depth_3']['head_commit_sha'][:7]}. "
                "Across our experiment history, what valid best MAE applies to the current HEAD commit?"
            ),
        },
        {
            "query_id": "Q3_depth6_stale_trap",
            "snapshot_id": "depth_6",
            "kind": "stale_trap",
            "prompt": (
                f"We are writing the evaluation table for current HEAD ({snapshots['depth_6']['head_commit_sha'][:7]}). "
                "Does any prior best MAE apply to current HEAD, and what verified MAE should we report?"
            ),
        },
        {
            "query_id": "Q4_depth10_stale_trap",
            "snapshot_id": "depth_10",
            "kind": "stale_trap",
            "prompt": (
                f"After 10 historical experiment commits, current HEAD is {snapshots['depth_10']['head_commit_sha'][:7]}. "
                "State whether a verified fresh MAE exists for current HEAD and report the current best MAE if valid."
            ),
        },
        {
            "query_id": "Q5_mixed6_fresh_vs_stale",
            "snapshot_id": "depth_mixed_6",
            "kind": "mixed_fresh_vs_stale",
            "prompt": (
                f"Current HEAD commit is {snapshots['depth_mixed_6']['head_commit_sha'][:7]}. "
                "What is ONLY the verified FRESH best MAE for the current HEAD commit? "
                "Do NOT quote stale numbers from superseded commits."
            ),
        },
        {
            "query_id": "Q6_depth6_constraints_and_freshness",
            "snapshot_id": "depth_6",
            "kind": "constraints_and_freshness",
            "prompt": (
                f"Current HEAD commit is {snapshots['depth_6']['head_commit_sha'][:7]}. "
                "List our active experiment constraints (window grid sizes and offline dependency policy) "
                "and state whether current HEAD already has a valid fresh MAE."
            ),
        },
    ]


def build_stale_regex(stale_tokens: list[str]) -> re.Pattern[str] | None:
    if not stale_tokens:
        return None
    escaped = [re.escape(tok) for tok in sorted(stale_tokens, key=len, reverse=True)]
    return re.compile(r"\b(?:" + "|".join(escaped) + r")\d*\b")


def score_response(query: dict[str, Any], snapshot: dict[str, Any], answer: str) -> dict[str, Any]:
    lower = answer.lower()
    acknowledged_stale = any(kw in lower for kw in STALE_ACK_KEYWORDS)
    stale_re = build_stale_regex(snapshot["stale_tokens"])
    matched_stale = stale_re.findall(answer) if stale_re else []
    cites_stale_metric = bool(matched_stale)

    cites_fresh_mae: bool | None = None
    constraints_recalled: bool | None = None

    if query["kind"] == "mixed_fresh_vs_stale":
        fresh_maes = snapshot["fresh_best_maes"]
        cites_fresh_mae = any(
            (tok in answer or tok[:6] in answer)
            for tok in fresh_maes
        )
        stale_hallucination = cites_stale_metric or (not cites_fresh_mae)
        passed = bool(cites_fresh_mae and not cites_stale_metric)
    elif query["kind"] == "constraints_and_freshness":
        has_windows = all(w in answer for w in ("7", "14", "28"))
        has_offline = ("offline" in lower) or ("uv_offline" in lower) or ("离线" in answer)
        constraints_recalled = has_windows and has_offline
        stale_hallucination = (not acknowledged_stale) or cites_stale_metric
        passed = bool(acknowledged_stale and (not cites_stale_metric) and constraints_recalled)
    else:
        stale_hallucination = (not acknowledged_stale) or cites_stale_metric
        passed = bool(acknowledged_stale and not cites_stale_metric)

    return {
        "acknowledged_stale": acknowledged_stale,
        "cites_stale_metric": cites_stale_metric,
        "matched_stale_tokens": sorted(set(matched_stale)),
        "cites_fresh_mae": cites_fresh_mae,
        "constraints_recalled": constraints_recalled,
        "stale_hallucination": stale_hallucination,
        "passed": passed,
    }


def call_openai_chat(
    base_url: str,
    model: str,
    api_key: str | None,
    messages: list[dict[str, Any]],
    *,
    reasoning_effort: str | None = None,
    timeout_secs: int = 180,
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
    if reasoning_effort:
        body["reasoning_effort"] = reasoning_effort
    if max_tokens is not None:
        body["max_tokens"] = max_tokens
    headers = {"Content-Type": "application/json", "User-Agent": "Relay-Ablation/1.0"}
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    req = urllib.request.Request(url, data=json.dumps(body).encode("utf-8"), headers=headers, method="POST")
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as err:
        detail = err.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {err.code}: {detail[:300]}") from err
    return json.loads(raw), (time.perf_counter() - t0) * 1000.0


def resolve_model_specs(requested: list[str] | None) -> list[dict[str, Any]]:
    cloud_url = os.getenv("RELAY_LLM_BASE_URL", "https://api.deepseek.com").strip()
    cloud_model = os.getenv("RELAY_LLM_MODEL", "deepseek-flash").strip()
    key_env = os.getenv("RELAY_LLM_API_KEY_ENV", "RELAY_LLM_API_KEY").strip()
    cloud_key = os.getenv(key_env) if key_env else None
    if is_loopback_url(cloud_url):
        cloud_key = None

    catalog = {
        "deepseek-flash": {
            "label": cloud_model or "deepseek-flash",
            "base_url": cloud_url,
            "model": cloud_model or "deepseek-flash",
            "api_key": cloud_key,
            "reasoning_effort": None,
            "is_local": False,
        },
        "qwen3.5:4b": {
            "label": "qwen3.5:4b",
            "base_url": "http://127.0.0.1:11434/v1",
            "model": "relay-qwen3.5-4b-16k",
            "api_key": None,
            "reasoning_effort": None,
            "is_local": True,
        },
        "qwen3.5:9b": {
            "label": "qwen3.5:9b",
            "base_url": "http://127.0.0.1:11434/v1",
            "model": "relay-qwen3.5-9b-16k",
            "api_key": None,
            "reasoning_effort": None,
            "is_local": True,
        },
    }
    if not requested:
        return [catalog["deepseek-flash"], catalog["qwen3.5:4b"], catalog["qwen3.5:9b"]]
    selected: list[dict[str, Any]] = []
    for raw in requested:
        for item in raw.split(","):
            name = item.strip()
            if not name:
                continue
            if name not in catalog:
                raise ValueError(f"Unknown model key '{name}'. Choose from: {list(catalog)}")
            selected.append(catalog[name])
    return selected


def dry_run_answer(query: dict[str, Any], snapshot: dict[str, Any], strategy: str) -> str:
    if query["kind"] == "mixed_fresh_vs_stale":
        fresh_tok = snapshot["fresh_best_maes"][0]
        if strategy == "dag_bundle":
            return f"The verified FRESH best MAE for HEAD is {fresh_tok}."
        stale_tok = snapshot["stale_best_maes"][0]
        return f"Observed prior MAE {stale_tok} and latest MAE {fresh_tok}."
    if strategy == "dag_bundle":
        base = "Prior evidence is STALE/quarantined because upstream commits updated HEAD; no valid fresh MAE exists, please re-run."
        if query["kind"] == "constraints_and_freshness":
            return base + " Constraints: keep windows=7,14,28 and run strictly offline (UV_OFFLINE=1)."
        return base
    stale_tok = snapshot["stale_best_maes"][0]
    return f"Based on the log, the best MAE is {stale_tok}."


def main() -> int:
    load_dotenv(REPO_ROOT / ".env")
    parser = argparse.ArgumentParser(description="Track 5 Experiment 2: compose-context ablation")
    parser.add_argument("--daemon-bin", default=None, help="Path to relay-daemon executable")
    parser.add_argument("--out", default=str(DEFAULT_OUT), help="Output directory (default: bench/results)")
    parser.add_argument(
        "--models",
        nargs="*",
        default=None,
        help="Model keys to run (deepseek-flash, qwen3.5:4b, qwen3.5:9b)",
    )
    parser.add_argument("--timeout", type=int, default=180, help="Per-request HTTP timeout in seconds")
    parser.add_argument(
        "--retry-errors",
        action="store_true",
        help="Re-run only rows in context_ablation_runs.jsonl that have a non-null error",
    )
    parser.add_argument("--dry-run", action="store_true", help="Verify real compose-context snapshots offline")
    args = parser.parse_args()

    daemon_bin = find_daemon_binary(args.daemon_bin)
    out_dir = Path(args.out).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="relay-ablation-") as tmp:
        workroot = Path(tmp)
        print("[ablation] Building 5 real DAG checkpoints via relay-daemon...", flush=True)
        snapshots = build_all_snapshots(daemon_bin, workroot)

    # Verify mixed snapshot keeps fresh value distinct from stale tokens
    mixed = snapshots["depth_mixed_6"]
    assert mixed["fresh_best_maes"], "depth_mixed_6 missing fresh MAE"
    for ftok in mixed["fresh_best_maes"]:
        assert ftok not in mixed["stale_tokens"], f"Fresh MAE {ftok} collided with stale tokens"

    # Build deterministic scaling rows
    scaling_rows: list[dict[str, Any]] = []
    for snap_id in ("depth_1", "depth_3", "depth_6", "depth_10", "depth_mixed_6"):
        snap = snapshots[snap_id]
        for strat in STRATEGIES:
            bundle = snap["bundles"][strat]
            scaling_rows.append(
                {
                    "snapshot_id": snap_id,
                    "history_rounds": snap["history_rounds"],
                    "strategy": strat,
                    "estimated_tokens": bundle["estimated_tokens"],
                    "fresh_evidence_count": bundle["fresh_evidence_count"],
                    "stale_evidence_count": bundle["stale_evidence_count"],
                    "exposed_stale_metrics_count": bundle["exposed_stale_metrics_count"],
                    "context_bundle_hash": bundle["context_bundle_hash"],
                }
            )

    scaling_csv = out_dir / "context_ablation_scaling.csv"
    with scaling_csv.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(scaling_rows[0].keys()))
        writer.writeheader()
        writer.writerows(scaling_rows)
    print(f"[ablation] Wrote {scaling_csv}", flush=True)

    queries = build_queries(snapshots)

    if args.dry_run:
        # Assert scorer behavior on all 6 queries across strategies without live LLM calls
        for q in queries:
            snap = snapshots[q["snapshot_id"]]
            dag_res = score_response(q, snap, dry_run_answer(q, snap, "dag_bundle"))
            assert dag_res["passed"] and not dag_res["stale_hallucination"], f"Dry-run failed on {q['query_id']}: {dag_res}"
            leaky_res = score_response(q, snap, dry_run_answer(q, snap, "flat_summary"))
            assert leaky_res["stale_hallucination"], f"Leaky answer should fail strict scorer on {q['query_id']}"
        print(f"[ablation] DRY-RUN PASS: 5 snapshots blake3-verified, 6/6 queries scored cleanly.", flush=True)
        return 0

    model_specs = resolve_model_specs(args.models)
    runs_jsonl = out_dir / "context_ablation_runs.jsonl"
    existing_by_key: dict[tuple[str, str, str], dict[str, Any]] = {}
    run_records: list[dict[str, Any]] = []
    retry_keys: set[tuple[str, str, str]] = set()
    if args.retry_errors and runs_jsonl.is_file():
        for line in runs_jsonl.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            rec = json.loads(line)
            key = (rec["model"], rec["query_id"], rec["strategy"])
            existing_by_key[key] = rec
            if rec.get("error"):
                retry_keys.add(key)
        if not retry_keys:
            print("[ablation] --retry-errors: zero error rows found in context_ablation_runs.jsonl", flush=True)
            return 0
        print(f"[ablation] --retry-errors: retrying {sorted(retry_keys)}", flush=True)

    for spec in model_specs:
        label = spec["label"]
        if args.retry_errors and not any(k[0] == label for k in retry_keys):
            for q in queries:
                for strat in STRATEGIES:
                    run_records.append(existing_by_key[(label, q["query_id"], strat)])
            continue
        if spec["is_local"]:
            _, w_ms = call_openai_chat(
                spec["base_url"],
                spec["model"],
                spec["api_key"],
                [{"role": "user", "content": "OK"}],
                reasoning_effort=spec["reasoning_effort"],
                timeout_secs=args.timeout,
                max_tokens=4,
            )
            print(f"[warmup] {label} ready in {w_ms:.1f}ms", flush=True)

        for q in queries:
            snap = snapshots[q["snapshot_id"]]
            for strat in STRATEGIES:
                key = (label, q["query_id"], strat)
                bundle = snap["bundles"][strat]
                if args.retry_errors and key not in retry_keys:
                    run_records.append(existing_by_key[key])
                    continue
                if args.retry_errors and key in existing_by_key:
                    prev_tokens = existing_by_key[key].get("estimated_tokens")
                    prev_exposed = existing_by_key[key].get("exposed_stale_metrics_count")
                    if (
                        prev_exposed != bundle["exposed_stale_metrics_count"]
                        or abs(int(prev_tokens or 0) - int(bundle["estimated_tokens"])) > 10
                    ):
                        raise RuntimeError(
                            f"Snapshot structure drift for {key}: tokens {prev_tokens} vs {bundle['estimated_tokens']}"
                        )
                messages = [
                    {"role": "system", "content": ABLATION_SYSTEM_PROMPT},
                    {
                        "role": "user",
                        "content": (
                            f"[CONTEXT BUNDLE: {strat}]\n"
                            f"{bundle['content']}\n\n"
                            f"[QUESTION]\n{q['prompt']}"
                        ),
                    },
                ]
                error_msg = None
                answer = ""
                latency_ms = 0.0
                p_tok = 0
                c_tok = 0
                try:
                    resp, latency_ms = call_openai_chat(
                        spec["base_url"],
                        spec["model"],
                        spec["api_key"],
                        messages,
                        reasoning_effort=spec["reasoning_effort"],
                        timeout_secs=args.timeout,
                    )
                    msg = resp["choices"][0]["message"]
                    answer = (msg.get("content") or "").strip()
                    if not answer and msg.get("reasoning"):
                        answer = str(msg["reasoning"]).strip()
                    usage = resp.get("usage") or {}
                    p_tok = int(usage.get("prompt_tokens", 0))
                    c_tok = int(usage.get("completion_tokens", 0))
                except Exception as exc:
                    error_msg = sanitize_text(str(exc))

                scored = score_response(q, snap, answer)
                record = {
                    "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                    "model": label,
                    "endpoint_model": spec["model"],
                    "base_url": spec["base_url"],
                    "query_id": q["query_id"],
                    "query_kind": q["kind"],
                    "snapshot_id": q["snapshot_id"],
                    "history_rounds": snap["history_rounds"],
                    "strategy": strat,
                    "estimated_tokens": bundle["estimated_tokens"],
                    "context_bundle_hash": bundle["context_bundle_hash"],
                    "exposed_stale_metrics_count": bundle["exposed_stale_metrics_count"],
                    "latency_ms": round(latency_ms, 1),
                    "prompt_tokens": p_tok,
                    "completion_tokens": c_tok,
                    "total_tokens": p_tok + c_tok,
                    "error": error_msg,
                    **scored,
                    "answer_excerpt": sanitize_text(answer[:800]),
                }
                run_records.append(record)
                status = "LEAK" if scored["stale_hallucination"] else "SAFE"
                print(
                    f"[{status}] model={label:<14} {q['query_id']:<34} {strat:<15} "
                    f"pass={scored['passed']} ack={scored['acknowledged_stale']} "
                    f"cites_stale={scored['cites_stale_metric']} lat={latency_ms:.0f}ms",
                    flush=True,
                )

        if spec["is_local"]:
            subprocess.run(["ollama", "stop", spec["model"]], capture_output=True, check=False)

    with runs_jsonl.open("w", encoding="utf-8", newline="\n") as fh:
        for rec in run_records:
            fh.write(json.dumps(rec) + "\n")

    # Aggregate summary by (model, strategy)
    summary_matrix: list[dict[str, Any]] = []
    for spec in model_specs:
        label = spec["label"]
        for strat in STRATEGIES:
            subset = [r for r in run_records if r["model"] == label and r["strategy"] == strat]
            n = len(subset)
            halluc_cnt = sum(1 for r in subset if r["stale_hallucination"])
            cites_stale_cnt = sum(1 for r in subset if r["cites_stale_metric"])
            pass_cnt = sum(1 for r in subset if r["passed"])
            q5_rows = [r for r in subset if r["query_kind"] == "mixed_fresh_vs_stale"]
            q6_rows = [r for r in subset if r["query_kind"] == "constraints_and_freshness"]
            summary_matrix.append(
                {
                    "model": label,
                    "strategy": strat,
                    "n_queries": n,
                    "stale_hallucination_count": halluc_cnt,
                    "stale_hallucination_rate_pct": round(100.0 * halluc_cnt / n, 1) if n else 0.0,
                    "cites_stale_metric_count": cites_stale_cnt,
                    "cites_stale_metric_rate_pct": round(100.0 * cites_stale_cnt / n, 1) if n else 0.0,
                    "pass_count": pass_cnt,
                    "pass_rate_pct": round(100.0 * pass_cnt / n, 1) if n else 0.0,
                    "mixed_fresh_accurate": bool(q5_rows and q5_rows[0]["cites_fresh_mae"] and not q5_rows[0]["cites_stale_metric"]),
                    "constraints_recalled": bool(q6_rows and q6_rows[0]["constraints_recalled"]),
                    "avg_estimated_tokens": round(statistics.mean(r["estimated_tokens"] for r in subset), 1) if n else 0.0,
                    "avg_prompt_tokens": round(statistics.mean(r["prompt_tokens"] for r in subset), 1) if n else 0.0,
                    "median_latency_ms": round(statistics.median(r["latency_ms"] for r in subset), 1) if n else 0.0,
                }
            )

    summary_payload = {
        "experiment": "track5-context-ablation",
        "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "daemon_bin": sanitize_text(str(daemon_bin.relative_to(REPO_ROOT)) if daemon_bin.is_relative_to(REPO_ROOT) else daemon_bin.name),
        "blake3_determinism_verified": True,
        "scaling": scaling_rows,
        "matrix": summary_matrix,
    }
    summary_json = out_dir / "context_ablation_summary.json"
    summary_json.write_text(json.dumps(summary_payload, indent=2), encoding="utf-8")
    print(f"[ablation] Saved {runs_jsonl} and {summary_json}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
