#!/usr/bin/env bash
# One-shot cross-platform entry point for the Track 4 Chaos Suite.
#
# Builds relay-daemon if missing, then executes run_chaos_suite.py (defaulting
# to --mode scripted with the P2P loopback gate enabled; pass --mode llm or any
# other flag through "$@").
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

EXE_EXT=""
case "${OS:-$(uname -s)}" in
    Windows_NT|MINGW*|MSYS*|CYGWIN*) EXE_EXT=".exe" ;;
esac

DAEMON_BIN="${RELAY_DAEMON_BIN:-target/debug/relay-daemon${EXE_EXT}}"

if [[ ! -f "$DAEMON_BIN" ]]; then
    echo "==> building relay-daemon (debug)"
    cargo build -p relay-daemon --locked
fi

echo "==> relay chaos suite"
if command -v uv >/dev/null 2>&1; then
    exec uv run python bench/chaos-cases/run_chaos_suite.py --daemon-bin "$DAEMON_BIN" "$@"
elif command -v python3 >/dev/null 2>&1; then
    exec python3 bench/chaos-cases/run_chaos_suite.py --daemon-bin "$DAEMON_BIN" "$@"
else
    exec python bench/chaos-cases/run_chaos_suite.py --daemon-bin "$DAEMON_BIN" "$@"
fi
