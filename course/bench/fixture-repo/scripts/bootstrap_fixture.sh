#!/usr/bin/env bash
# Materialize course/bench/fixture-repo as a standalone Git checkout that Relay can execute.
#
# The fixture lives inside the Relay monorepo, so Relay's worktree sandbox cannot
# treat it as its own repository. This script copies it to a scratch directory,
# initializes Git, and commits the baseline exactly once.
#
# Usage: course/bench/fixture-repo/scripts/bootstrap_fixture.sh [target-dir]
set -euo pipefail

SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET_DIR="${1:-$SOURCE_DIR/../../../.tmp/relay-fixture}"
mkdir -p "$(dirname "$TARGET_DIR")"
TARGET_DIR="$(cd "$(dirname "$TARGET_DIR")" && pwd)/$(basename "$TARGET_DIR")"

if [ -e "$TARGET_DIR/.git" ]; then
  echo "fixture already initialized at $TARGET_DIR" >&2
  exit 0
fi

mkdir -p "$TARGET_DIR"
tar -C "$SOURCE_DIR" \
  --exclude .git --exclude .venv --exclude .relay --exclude __pycache__ \
  -cf - . | tar -C "$TARGET_DIR" -xf -

git -C "$TARGET_DIR" init --initial-branch=main >/dev/null
git -C "$TARGET_DIR" config core.autocrlf false
git -C "$TARGET_DIR" config user.name "Relay Fixture"
git -C "$TARGET_DIR" config user.email "fixture@relay.local"
git -C "$TARGET_DIR" add --all
git -C "$TARGET_DIR" commit -m "chore(bench): initialize forecast fixture" >/dev/null

echo "fixture ready: $TARGET_DIR"
echo "export RELAY_REPO_ROOT=$TARGET_DIR"
