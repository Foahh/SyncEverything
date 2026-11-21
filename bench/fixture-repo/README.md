# Relay Rolling-Window Forecast Fixture (`bench/fixture-repo`)

Standard reproducible Python benchmark project for Relay v1.0 demos and Chaos Suite evaluation:

- **Smoke preflight (`relay.toml` → `smoke`)**: `< 1s` single-window check (`uv run python src/forecast.py --smoke`)
- **3-Window Grid Sweep (`relay.toml` → `train_grid`)**: evaluates rolling windows `7, 14, 28` and outputs structured JSON metrics (`best_window`, `best_mae`, per-window MAE/RMSE)
- **Dependency lockfile**: `pyproject.toml` + `uv.lock`
