# Relay Rolling-Window Forecast Fixture (`course/bench/fixture-repo`)

Standard reproducible Python benchmark project for Relay v1.0 demos and the Track 4 Chaos Suite. The fixture is deliberately dependency-free and offline-safe: every preset runs from the Python standard library plus one vendored wheel.

## Presets (`relay.toml`)

| Preset | Command | Budget | Purpose |
|---|---|---|---|
| `smoke` | `uv run python src/forecast.py --smoke` | 30s timeout, measured `< 0.3s` warm | Preflight gate used by every self-healing case |
| `train_grid` | `uv run python src/forecast.py --windows 7,14,28` | 120s timeout, measured `< 1s` | 3-window hyperparameter grid, emits `best_window` / `best_mae` / per-window MAE+RMSE |
| `alpha_sweep` | `uv run python src/forecast.py --windows 7,14,28 --alphas 0.2,0.35,0.5` | 120s timeout, measured `< 1s` | 3x3 calibration replay for reproducibility checks |

All presets accept only the parameters listed in `allowed_params`. The engine appends `--<key> <value>` for each approved parameter.

## Calibration gates

Run `python scripts/verify_calibration.py` (or pass `--repo <checkout>`). The script verifies four gates offline:

| Gate | Check | Current result |
|---|---|---|
| 1 | 3 smoke runs all finish under 3s | worst 649 ms, median 228 ms |
| 2 | `train_grid` output reproducible except `elapsed_ms`; `best_window = 28` | `best_mae = 2.16502`, 3 rows |
| 3 | `alpha_sweep` reproducible, 9 rows (3 windows x 3 alphas) | best `window=28 alpha=0.5 mae=1.92274` |
| 4 | Preset runs leave checkout clean & vendored wheel installs offline in an isolated temp copy | `tiny_stats.mean([1,2,3]) = 2.0` (`--repo` untouched) |

`elapsed_ms` is wall-clock telemetry and is excluded from the reproducibility comparison, both here and in the Track 5 plot data. The latest full run is recorded in `results/calibration-latest.txt` (four gates, `PASS`).

## Offline dependency healing

`vendor/tiny_stats-0.1.0-py3-none-any.whl` is a tiny pure-Python helper built by `scripts/build_vendor_wheel.py` (deterministic entry timestamps). The missing-dependency Chaos case injects `import tiny_stats`, then repairs it with a `dependency_spec` patch pointing at this wheel:

```
./vendor/tiny_stats-0.1.0-py3-none-any.whl
```

Because the wheel is local, `uv add` and `uv run` never touch an index. The suite exports `UV_OFFLINE=1`, `UV_NO_CONFIG=1`, and clears `UV_DEFAULT_INDEX` so a machine-local mirror cannot leak into the proposal diff.

## Scripts

| Script | Purpose |
|---|---|
| `scripts/bootstrap_fixture.sh [target]` | Copy the fixture into a standalone Git checkout (default `.tmp/relay-fixture`) and commit the baseline |
| `scripts/warm_offline_cache.sh [repo]` | Create `.venv`, run smoke + grid, and verify the vendor wheel with networking disabled |
| `scripts/verify_calibration.py` | Assert the four calibration gates above |
| `scripts/build_vendor_wheel.py` | Regenerate the deterministic vendor wheel |
| `results/calibration-latest.txt` | Output of the last `verify_calibration.py` run (committed as Track 4 evidence) |

## Running a preset by hand

```bash
course/bench/fixture-repo/scripts/bootstrap_fixture.sh
export RELAY_REPO_ROOT=$PWD/.tmp/relay-fixture
uv run --directory "$RELAY_REPO_ROOT" python src/forecast.py --windows 7,14,28
```

Relay itself refuses to execute the fixture while it is embedded in this monorepo (the worktree sandbox requires the repository root to own `.git`). Always materialize a standalone checkout first.

## Environment requirements

- Python 3.11+ (declared in `pyproject.toml`); no PyPI packages required.
- `uv` on `PATH`.
- Git on `PATH`.

The fixture is intentionally insensitive to `alpha`, `window` ordering, and wall-clock noise so the judge can compare metrics exactly across machines.
