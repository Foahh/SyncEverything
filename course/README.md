# Academic & Course Archive

This directory consolidates the original research, course planning, benchmark, and evaluation artifacts produced during the initial development of SyncEverything (powered by the Relay engine).

## Directory Index

| Directory | Contents |
| --- | --- |
| [`proposal/`](proposal/) | Original project proposal ([`relay_proposal.md`](proposal/relay_proposal.md) and [`COMP 7608 Project Proposal.pdf`](proposal/COMP%207608%20Project%20Proposal.pdf)). |
| [`planning/`](planning/) | Historical development plan, milestone breakdown, and evaluation tracking ([`relay_plan.md`](planning/relay_plan.md)). |
| [`demo/`](demo/) | Dual-mode demonstration notes and checklists ([`README.md`](demo/README.md)). |
| [`bench/`](bench/) | Reproducible forecasting fixture ([`fixture-repo/`](bench/fixture-repo/)), fault-injection Chaos Suite ([`chaos-cases/`](bench/chaos-cases/)), and baseline/ablation evaluation runners, logs, and figures ([`results/`](bench/results/)). |
| [`eval/`](eval/) | Live MCP model evaluation harness, stale-metric scoring helpers, and regression unit tests ([`models/`](eval/models/), [`test_release_scoring.py`](eval/test_release_scoring.py)). |

## Running the Harness Unit Tests

From the repository root:

```bash
uv run python -m unittest discover -s course/eval -p "test_*.py"
uv run python -m unittest discover -s course/bench/results -p "test_*.py"
```
