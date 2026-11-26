# Benchmarks & Evaluation Suite

This directory consolidates the reproducible benchmark and evaluation artifacts for SyncEverything.

## Directory Index

| Directory | Contents |
| --- | --- |
| [`bench/`](bench/) | Reproducible forecasting fixture ([`fixture-repo/`](bench/fixture-repo/)), fault-injection Chaos Suite ([`chaos-cases/`](bench/chaos-cases/)), and baseline/ablation evaluation runners, logs, and figures ([`results/`](bench/results/)). |
| [`eval/`](eval/) | Live MCP model evaluation harness, stale-metric scoring helpers, and regression unit tests ([`models/`](eval/models/), [`test_release_scoring.py`](eval/test_release_scoring.py)). |

## Running the Harness Unit Tests

From the repository root:

```bash
uv run python -m unittest discover -s experiments/eval -p "test_*.py"
uv run python -m unittest discover -s experiments/bench/results -p "test_*.py"
```
