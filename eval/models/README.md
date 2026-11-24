# Model Evaluation Matrix & Live MCP Harness (Track 3)

Benchmark harness evaluating **any OpenAI-compatible model** (local Ollama `qwen3.5:4b` / `qwen3.5:9b`, DeepSeek API, OpenAI, OpenRouter, etc.) directly against the live `relay-daemon mcp-stdio` (`rmcp` 3.11) server across 4 standardized tool-calling scenarios:

1. **`S1_sync_and_handoffs`**: Read-only `inspect_sync_status` + `list_handoffs` selection and grounding.
2. **`S2_stale_evidence_trap`**: Deterministic `STALE` cascade after an upstream Git commit; enforces a strict stale-metric check requiring the model to BOTH acknowledge staleness AND withhold quarantined stale numeric values (`2.165...`) from its answer.
3. **`S3_env_diff_diagnosis`**: Commit-vs-Host `check_env_diff` fingerprint drift detection.
4. **`S4_code_defect_self_heal`**: End-to-end isolated `git worktree` preflight (`NameError`) → verified `code_diff` via `propose_worktree_patch` → human-in-the-loop `approval_required` gate without dirtying the main working tree.

---

## Quick Usage

### 1. Offline Harness Verification (No LLM Required)
Verify the real `relay-daemon mcp-stdio` JSON-RPC 2.0 handshake, all 8 tools, `STALE` quarantine, and worktree self-healing:

```powershell
cargo build -p relay-daemon
uv run python eval/models/run_mcp_eval.py --dry-run
```

### 2. Benchmark Local Ollama (`qwen3.5:4b` / `qwen3.5:9b`)

> **Important for Ollama `/v1/chat/completions`:** Ollama defaults `/v1` requests to a 4096-token context window (ignoring per-request `options.num_ctx`), which truncates multi-turn tool traces such as `S4_code_defect_self_heal`. Create a temporary 2-line zero-copy alias with `num_ctx 16384` before running:
>
> ```powershell
> "FROM qwen3.5:4b`nPARAMETER num_ctx 16384" | Set-Content "$env:TEMP\Modelfile.4b"
> ollama create relay-qwen3.5-4b-16k -f "$env:TEMP\Modelfile.4b"
> ```
>
> On an 8 GB VRAM GPU without `OLLAMA_KV_CACHE_TYPE=q8_0`, `qwen3.5:4b` at 16K context fits `100% GPU`, whereas `qwen3.5:9b` spills ~16% of layers to CPU (`16%/84% CPU/GPU`). Keep the harness default of preserving assistant `reasoning` fields across tool turns—stripping `reasoning` causes Ollama's Qwen3.5 thinking parser to emit empty `content` on follow-up turns.

```powershell
# Fast / non-thinking edge baseline (Edge Fast-Path)
uv run python eval/models/run_mcp_eval.py `
  --base-url http://127.0.0.1:11434/v1 `
  --model relay-qwen3.5-4b-16k `
  --label "qwen3.5:4b (no-think)" `
  --reasoning-effort none `
  --warmup --save

# Default thinking mode (replace with relay-qwen3.5-9b-16k and label "qwen3.5:9b" for 9B)
uv run python eval/models/run_mcp_eval.py `
  --base-url http://127.0.0.1:11434/v1 `
  --model relay-qwen3.5-4b-16k `
  --label "qwen3.5:4b" `
  --warmup --save
```

### 3. Benchmark Any Cloud OpenAI-Compatible API (e.g., DeepSeek / OpenAI)

```powershell
$env:DEEPSEEK_API_KEY = "<your-key>"
uv run python eval/models/run_mcp_eval.py `
  --base-url https://api.deepseek.com/v1 `
  --model deepseek-chat `
  --api-key-env DEEPSEEK_API_KEY `
  --warmup --save
```

### Useful Harness Flags & Safety

- `--label`: Display name written to `results/*.json` and `benchmark-report.md` (defaults to `--model`).
- `--reasoning-effort {none,low,medium,high}`: Forward `reasoning_effort` to endpoints that support toggling thinking mode.
- `--warmup` & `--timeout <secs>`: Send one unmeasured warmup request before timing scenarios and cap per-request HTTP latency (default `240s`).
- **Loopback API-key safety**: When `--base-url` targets `127.0.0.1`, `localhost`, or `::1`, cloud credentials loaded from `.env` (`RELAY_LLM_API_KEY`) are never forwarded unless `--api-key-env` is passed explicitly.

Passing `--save` stores the structured run log in `eval/models/results/` and updates `eval/models/benchmark-report.md` with the Markdown comparison table.

New runs record `scoring_version: 2`. The model report and Figure 3 use only live runs scored with the current version once the first such run exists. Until then, they display the original live cohort. Older JSON files remain available for reproducing the historical report.
