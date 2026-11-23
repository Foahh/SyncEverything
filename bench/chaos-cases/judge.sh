#!/usr/bin/env bash
# One-shot entry point for the Track 4 chaos suite.
#
# Builds the daemon if it is missing, then runs every ChaosCase and writes
# bench/chaos-cases/results/chaos-report.{json,md}. Defaults to the fastest
# local path; keep `--keep` (passed through) to retain per-case scratch repos.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

DAEMON_BIN="${RELAY_DAEMON_BIN:-target/debug/relay-daemon}"

if [[ ! -x "$DAEMON_BIN" ]]; then
    echo "==> building relay-daemon (debug)"
    cargo build -p relay-daemon --locked
fi

echo "==> relay chaos suite"
exec python3 bench/chaos-cases/run_chaos_suite.py --daemon-bin "$DAEMON_BIN" "$@"
