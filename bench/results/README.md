# Benchmark Results & Analysis (Track 5)

Raw experiment logs (CSV/JSONL/JSON), reproducible benchmark runners, consolidated report, and publication-grade figures (300 DPI PNG + vector SVG + PDF) for Relay v1.0.

## Artifact Inventory

| Experiment | Script | Outputs | Report / Figure |
|---|---|---|---|
| **Exp 1 · Three-Baseline Comparison** | [run_baseline_comparison.py](./run_baseline_comparison.py) | [baseline_comparison_runs.jsonl](./baseline_comparison_runs.jsonl)<br/>[baseline_comparison_summary.json](./baseline_comparison_summary.json)<br/>[baseline_comparison_summary.csv](./baseline_comparison_summary.csv) | [fig1_baseline_comparison.png](./figures/fig1_baseline_comparison.png) |
| **Exp 2 · Context Representation Ablation** | [run_context_ablation.py](./run_context_ablation.py) | [context_ablation_scaling.csv](./context_ablation_scaling.csv)<br/>[context_ablation_runs.jsonl](./context_ablation_runs.jsonl)<br/>[context_ablation_summary.json](./context_ablation_summary.json) | [fig2_context_ablation.png](./figures/fig2_context_ablation.png) |
| **Exp 3 · Local vs. Cloud Pareto (N=3)** | [eval/models/run_mcp_eval.py](../../eval/models/run_mcp_eval.py) | [eval/models/results/](../../eval/models/results/) (`N=3` × 5 configs)<br/>[eval/models/benchmark-report.md](../../eval/models/benchmark-report.md) | [fig3_model_pareto.png](./figures/fig3_model_pareto.png) |
| **Exp 4 · Chaos Suite Fault Breakdown** | [bench/chaos-cases/run_chaos_suite.py](../chaos-cases/run_chaos_suite.py) | [bench/chaos-cases/results/](../chaos-cases/results/) | [fig4_chaos_self_healing.png](./figures/fig4_chaos_self_healing.png) |
| **Consolidated Summary** | [plot_figures.py](./plot_figures.py) | `bench/results/figures/*.{png,svg,pdf}` | [experiment-report.md](./experiment-report.md) |

---

## Reproduction Commands

### 1. Offline Verification Gates (Zero LLM / Network Required)
```powershell
cargo build -p relay-daemon
uv run python bench/results/run_baseline_comparison.py --daemon-bin target/debug/relay-daemon.exe --dry-run
uv run python bench/results/run_context_ablation.py --daemon-bin target/debug/relay-daemon.exe --out "$env:TEMP\relay-ablation-check" --dry-run
uv run python eval/models/run_mcp_eval.py --daemon-bin target/debug/relay-daemon.exe --dry-run
```

### 2. Re-Generate All 4 Publication Figures (PNG + SVG + PDF)
```powershell
uv run --with matplotlib --with numpy python bench/results/plot_figures.py
```

### 3. Re-Run Live Experiments (Uses `.env` `RELAY_LLM_*` + Local 16K Ollama Aliases)
```powershell
# Experiment 1: Three-baseline comparison (N=3 trials x 3 baselines x 4 tasks = 36 runs)
uv run python bench/results/run_baseline_comparison.py --daemon-bin target/debug/relay-daemon.exe --trials 3 --out bench/results

# Experiment 2: Context representation ablation (create 16K Ollama aliases first per eval/models/README.md)
uv run python bench/results/run_context_ablation.py --daemon-bin target/debug/relay-daemon.exe --out bench/results
```

The ablation runner saves the verbatim contexts, prompts and scoring inputs in
`context_ablation_inputs.json`. Every result row records the SHA-256 of this file
and the context bundle's BLAKE3 hash. Keep it with the JSONL and scaling CSV.
To retry failed requests against those same inputs, use `--retry-errors` with
the same output directory. Add `--models` to retry a particular model. Other
models' rows stay in the dataset. Older runs without saved inputs require a
full rerun. Network errors are recorded separately and must be resolved before
generating Figure 2.

### 4. Underlying `relay-daemon` CLI Primitives (`doctor`, `compose-context`)
```powershell
cargo run -q -p relay-daemon -- --repo-root bench/fixture-repo doctor --json --skip-llm
cargo run -q -p relay-daemon -- --data-dir <DATA_DIR> --repo-root <REPO_PATH> compose-context --all-strategies --no-sync
```
