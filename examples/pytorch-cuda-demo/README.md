# IMU Gesture Classifier — PyTorch CUDA Demo (`SyncEverything` / `Relay`)

Lightweight, seeded PyTorch edge-compression demo designed for two-machine demos between a thin laptop (macOS Client) and a GPU workstation (Windows CUDA Host).

- **Scenario**: Compress a 6-axis wearable IMU 4-class gesture classifier (`idle` / `walk` / `wave` / `tap`) by slashing `param_count` while holding `val_acc >= 0.95`.
- **100% deterministic & offline-ready**: Synthesizes the 6-axis IMU dataset in memory from a fixed `--seed` with zero external downloads.
- **Automatic accelerator fallback**: Picks **CUDA -> Apple MPS -> CPU** unless overridden via `--device`.
- **Structured DAG Evidence**: Emits `val_acc`, `param_count`, `model_kb`, `val_loss`, `infer_ms`, `device`, `device_name`, and `elapsed_ms` as JSON to stdout.

## Presets (`relay.toml`)

| Preset | Purpose | Allowed Params |
|---|---|---|
| `smoke` | 1-step forward/backward & accelerator check | `device`, `hidden` |
| `train` | Baseline 3-layer GELU MLP (`hidden=128`, `17,924` params) | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |
| `train_compact` | Compressed edge MLP (`hidden=32`, `1,412` params, `-92%`) | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |
| `eval` | Fast 15-epoch validation sweep (`hidden=64`) | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |

## Quick Two-Machine Setup

1. On the Host, copy the three project files into a standalone directory and initialize Git. While online, run `uv sync` with your platform's PyTorch index (for example, CUDA wheels on Windows RTX GPUs) so `uv`'s local cache holds the wheels for isolated worktrees.

```powershell
New-Item -ItemType Directory -Path ~/pytorch-cuda-demo
Copy-Item examples/pytorch-cuda-demo/train.py,examples/pytorch-cuda-demo/pyproject.toml,examples/pytorch-cuda-demo/relay.toml ~/pytorch-cuda-demo
Set-Location ~/pytorch-cuda-demo
git init -b main
git add train.py pyproject.toml relay.toml
git commit -m "init imu gesture classifier demo"
uv sync
uv run --offline python train.py --smoke
```

> Note: `uv.lock` is intentionally omitted from the starter files so Windows CUDA workstations and Apple Silicon macOS laptops can each resolve their native PyTorch wheels out of the box. For Windows RTX 50-series (`sm_120`) GPUs, set `UV_EXTRA_INDEX_URL=https://download.pytorch.org/whl/cu132` (or your matching CUDA wheel index) when warming the cache and starting the Host daemon.

2. Clone this same commit on the laptop. Start the Host daemon (use `--lan-only` for air-gapped LAN, or omit it for campus Wi-Fi automatic `iroh` relay fallback):

```powershell
$env:UV_OFFLINE = "1"
relay-daemon --repo-root . serve
```

3. Open **SyncEverything** Desktop on the laptop, open the demo repository, paste the pairing ticket in **Devices**, and dispatch `train`. To demonstrate both `STALE` invalidation and cross-device self-healing, edit `train.py` on the laptop (for example, lowering `DEFAULT_HIDDEN` or introducing a hardcoded `select_device("mps")` bug) and commit.
