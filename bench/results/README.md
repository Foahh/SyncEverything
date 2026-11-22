# Benchmark Results & Analysis (Track 5)

Store raw CSV/JSONL experiment logs and publication-grade plotting scripts (Matplotlib/Plotly) here for:
1. Three-baseline comparison (File sync vs. Static template vs. Relay v1.0)
2. Context representation ablation (`full_transcript` vs. `flat_summary` vs. `dag_bundle`)
3. Local Ollama (`qwen3.5:4b/9b`) vs. Cloud API Pareto analysis (see `eval/models/run_mcp_eval.py`)
4. Chaos Suite 8-fault self-healing success rate & retry breakdown

---

## Reproducible CLI Primitives (`relay-daemon`)

### 1. Environment Pre-Check (`doctor`)
Verify required external runtimes (`git`, `uv`), repository `relay.toml`, and optional LLM endpoint before running benchmarks or dual-machine QA:

```powershell
cargo run -q -p relay-daemon -- --repo-root <REPO_PATH> doctor
cargo run -q -p relay-daemon -- --repo-root <REPO_PATH> doctor --json --skip-llm
```

### 2. Deterministic Context Ablation (`compose-context`)
Generate all three context representations from the exact same SQLite DAG snapshot, complete with `estimated_tokens`, `context_bundle_hash` (`blake3:...`), and `exposed_stale_metrics_count`:

```powershell
# All 3 strategies side-by-side (JSON)
cargo run -q -p relay-daemon -- --data-dir <DATA_DIR> --repo-root <REPO_PATH> compose-context --all-strategies --no-sync

# Single strategy (`full-transcript` | `flat-summary` | `dag-bundle`)
cargo run -q -p relay-daemon -- --data-dir <DATA_DIR> --repo-root <REPO_PATH> compose-context --strategy dag-bundle --no-sync
```
