#!/usr/bin/env python3
"""Verify the Track 4 fixture calibration gates.

Gates (all offline, no PyPI access):
1. Smoke preflight finishes in under 3 seconds on a warm environment.
2. The 3-window grid is byte-for-byte reproducible apart from elapsed_ms.
3. The alpha sweep is reproducible and covers 3 windows x 3 alphas.
4. Preset runs leave the repository clean AND the vendored tiny_stats wheel installs offline (tested in an isolated temp copy so --repo is never dirtied).

Usage:
    python course/bench/fixture-repo/scripts/verify_calibration.py
    python course/bench/fixture-repo/scripts/verify_calibration.py --repo /path/to/fixture-checkout
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

FIXTURE_SRC = Path(__file__).resolve().parents[1]
SMOKE_BUDGET_SECS = 3.0
REPEATS = 3
TEXT_SUFFIXES = {".py", ".toml", ".lock", ".md", ".sh", ".txt", ".json"}


def normalize_lf(root: Path) -> None:
    """Normalize text line endings so Windows CRLF checkouts stay clean under autocrlf=false."""
    for file_path in root.rglob("*"):
        if file_path.is_file() and file_path.suffix.lower() in TEXT_SUFFIXES:
            text = file_path.read_text(encoding="utf-8")
            file_path.write_text(text, encoding="utf-8", newline="\n")


def offline_env() -> dict[str, str]:
    env = os.environ.copy()
    for key in ("UV_DEFAULT_INDEX", "UV_INDEX", "UV_INDEX_URL", "UV_EXTRA_INDEX_URL"):
        env.pop(key, None)
    env.update({"UV_OFFLINE": "1", "UV_NO_CONFIG": "1", "UV_PYTHON_DOWNLOADS": "never", "UV_LINK_MODE": "copy"})
    return env


def run(args: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args, cwd=str(cwd), text=True, encoding="utf-8", errors="replace", capture_output=True, env=offline_env()
    )


def git_dirty(cwd: Path) -> bool:
    proc = run(["git", "status", "--porcelain"], cwd)
    proc.check_returncode()
    return proc.stdout.strip() != ""


def canonical(payload: dict) -> str:
    payload = dict(payload)
    payload.pop("elapsed_ms", None)
    return json.dumps(payload, sort_keys=True, separators=(",", ":"))


def preset(cwd: Path, *args: str) -> dict:
    proc = run(["uv", "run", "--offline", "python", "src/forecast.py", *args], cwd)
    if proc.returncode != 0:
        raise SystemExit(f"preset failed ({proc.returncode}):\n{proc.stdout}\n{proc.stderr}")
    return json.loads(proc.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify Relay fixture calibration gates")
    parser.add_argument("--repo", default=None, help="Existing fixture checkout (defaults to a temp copy)")
    args = parser.parse_args()

    if args.repo:
        repo = Path(args.repo).resolve()
        cleanup = None
    else:
        cleanup = tempfile.TemporaryDirectory(prefix="relay-fixture-calibration-")
        repo = Path(cleanup.name) / "repo"
        shutil.copytree(
            FIXTURE_SRC,
            repo,
            ignore=shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc"),
        )
        normalize_lf(repo)
        run(["git", "init", "--initial-branch=main"], repo).check_returncode()
        run(["git", "config", "core.autocrlf", "false"], repo).check_returncode()
        run(["git", "config", "user.name", "Relay Calibration"], repo).check_returncode()
        run(["git", "config", "user.email", "calibration@relay.local"], repo).check_returncode()
        run(["git", "add", "--all"], repo).check_returncode()
        run(["git", "-c", "commit.gpgsign=false", "commit", "-m", "chore(bench): calibration baseline"], repo).check_returncode()

    wheel_cleanup = tempfile.TemporaryDirectory(prefix="relay-fixture-wheel-gate-")
    try:
        failures: list[str] = []
        print(f"Fixture: {repo if args.repo else '<temporary copy>'}")

        sync = run(["uv", "sync", "--offline"], repo)
        if sync.returncode != 0:
            failures.append(f"uv sync --offline failed: {sync.stderr.strip()[-400:]}")
        if git_dirty(repo):
            failures.append("uv sync left the working tree dirty")

        # Gate 1 — smoke budget.
        durations = []
        for _ in range(REPEATS):
            started = time.perf_counter()
            report = preset(repo, "--smoke")
            durations.append(time.perf_counter() - started)
            if report.get("mode") != "smoke":
                failures.append("smoke preset reported a non-smoke mode")
            if git_dirty(repo):
                failures.append("smoke preset dirtied the working tree")
        worst = max(durations)
        print(f"smoke: {REPEATS} runs, median {statistics.median(durations) * 1000:.0f} ms, worst {worst * 1000:.0f} ms")
        if worst >= SMOKE_BUDGET_SECS:
            failures.append(f"smoke worst case {worst:.2f}s exceeds the {SMOKE_BUDGET_SECS:.0f}s budget")

        # Gate 2 — 3-window grid reproducibility.
        first = preset(repo, "--windows", "7,14,28")
        second = preset(repo, "--windows", "7,14,28")
        if canonical(first) != canonical(second):
            failures.append("train_grid output is not reproducible across runs")
        if first.get("best_window") != 28:
            failures.append(f"expected best_window=28 on the calibrated fixture, got {first.get('best_window')}")
        if len(first.get("runs", [])) != 3:
            failures.append("train_grid did not emit 3 window rows")
        if git_dirty(repo):
            failures.append("train_grid dirtied the working tree")
        print(f"grid: best_window={first.get('best_window')} best_mae={first.get('best_mae')} reproducible=OK")

        # Gate 3 — alpha sweep reproducibility.
        third = preset(repo, "--alphas", "0.2,0.35,0.5")
        fourth = preset(repo, "--alphas", "0.2,0.35,0.5")
        if canonical(third) != canonical(fourth):
            failures.append("alpha_sweep output is not reproducible across runs")
        if len(third.get("runs", [])) != 9:
            failures.append("alpha_sweep did not emit 3 windows x 3 alphas = 9 rows")
        if git_dirty(repo):
            failures.append("alpha_sweep dirtied the working tree")
        print(f"alpha sweep: rows={len(third.get('runs', []))} best={third.get('best')} reproducible=OK")

        # Gate 4 — offline vendor wheel installs and imports cleanly inside an isolated copy.
        wheel_repo = Path(wheel_cleanup.name) / "repo"
        shutil.copytree(
            repo,
            wheel_repo,
            ignore=shutil.ignore_patterns(".git", ".venv", ".relay", "__pycache__", "*.pyc"),
        )
        add = run(["uv", "add", "--no-sync", "--", "./vendor/tiny_stats-0.1.0-py3-none-any.whl"], wheel_repo)
        if add.returncode != 0:
            failures.append(f"offline uv add of the vendor wheel failed: {add.stderr.strip()[-400:]}")
        add_smoke = run(
            ["uv", "run", "--offline", "python", "-c", "import tiny_stats; print(tiny_stats.mean([1, 2, 3]))"],
            wheel_repo,
        )
        if add_smoke.returncode != 0:
            failures.append(f"vendored tiny_stats import failed: {add_smoke.stderr.strip()[-400:]}")
        print(f"vendor wheel: import OK (mean={add_smoke.stdout.strip()})")

        if failures:
            print("\nFAIL")
            for failure in failures:
                print(f"- {failure}")
            return 1
        print("\nPASS: 4 calibration gates")
        return 0
    finally:
        wheel_cleanup.cleanup()
        if cleanup is not None:
            cleanup.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
