#!/usr/bin/env bash
# Warm the offline runtime for the fixture: create the uv environment and prove
# that smoke, the grid, and the vendored dependency all work with no network.
#
# Usage: experiments/bench/fixture-repo/scripts/warm_offline_cache.sh [repo-dir]
set -euo pipefail

REPO_DIR="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_DIR"

# Keep uv away from machine-local mirrors and force it offline.
unset UV_DEFAULT_INDEX UV_INDEX UV_INDEX_URL UV_EXTRA_INDEX_URL PIP_INDEX_URL PIP_EXTRA_INDEX_URL
export UV_OFFLINE=1
export UV_NO_CONFIG=1
export UV_PYTHON_DOWNLOADS=never
export UV_LINK_MODE=copy

uv sync --offline
uv run --offline python src/forecast.py --smoke >/dev/null
uv run --offline python src/forecast.py --windows 7,14,28 >/dev/null
uv run --offline python - <<'PY'
import pathlib, zipfile
wheel = pathlib.Path("vendor/tiny_stats-0.1.0-py3-none-any.whl")
with zipfile.ZipFile(wheel) as archive:
    assert "tiny_stats/__init__.py" in archive.namelist()
print(f"vendor wheel ready: {wheel} ({wheel.stat().st_size} bytes)")
PY

echo "offline runtime warm: $REPO_DIR/.venv"
