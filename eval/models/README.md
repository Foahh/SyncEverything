# Model Evaluation Matrix & Live MCP Harness (Track 3)

Benchmark harness evaluating **any OpenAI-compatible model** (local Ollama `qwen3.5:4b` / `qwen3.5:9b`, DeepSeek API, OpenAI, OpenRouter, etc.) directly against the live `relay-daemon mcp-stdio` (`rmcp` 3.11) server across 4 standardized tool-calling scenarios:

1. **`S1_sync_and_handoffs`**: Read-only `inspect_sync_status` + `list_handoffs` selection and grounding.
2. **`S2_stale_evidence_trap`**: Deterministic `STALE` cascade after an upstream Git commit; verifies 0% stale-metric hallucination.
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

```powershell
uv run python eval/models/run_mcp_eval.py `
  --base-url http://127.0.0.1:11434/v1 `
  --model qwen3.5:4b `
  --save
```

### 3. Benchmark Any Cloud OpenAI-Compatible API (e.g., DeepSeek / OpenAI)

```powershell
$env:DEEPSEEK_API_KEY = "<your-key>"
uv run python eval/models/run_mcp_eval.py `
  --base-url https://api.deepseek.com/v1 `
  --model deepseek-chat `
  --api-key-env DEEPSEEK_API_KEY `
  --save
```

Passing `--save` stores the structured run log in `eval/models/results/` and updates `eval/models/benchmark-report.md` with the Markdown comparison table.
