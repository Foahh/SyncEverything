# PyTorch CUDA Demo (`SyncEverything` / `Relay`)

Lightweight, deterministic 5-second PyTorch training & evaluation repository designed for cross-device live demos (e.g. laptop Client -> Windows/Linux GPU Workstation Host over `iroh` P2P LAN).

- Zero external downloads: synthesizes a 4-class spiral classification dataset in memory using a fixed `--seed`.
- Automatic accelerator fallback: picks **CUDA -> Apple MPS -> CPU** unless overridden via `--device`.
- Emits structured JSON metrics (`val_acc`, `val_loss`, `device`, `device_name`, `samples_per_sec`, `elapsed_ms`) captured into the Decision-Evidence DAG.

## Presets (`relay.toml`)

| Preset | Purpose | Allowed Params |
|---|---|---|
| `smoke` | 1-step forward/backward & accelerator check | `device` |
| `train` | 40-epoch 3-layer GELU MLP training run (~1-4s) | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |
| `eval` | Fast 15-epoch validation sweep | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |

## Quick Two-Machine LAN Setup

1. Copy this directory into its own standalone Git repository on both machines (or clone from your repo):

```powershell
mkdir -p ~/pytorch-cuda-demo
cp examples/pytorch-cuda-demo/* ~/pytorch-cuda-demo/
cd ~/pytorch-cuda-demo
git init -b main
git add .
git commit -m "init pytorch cuda demo"
```

2. On the GPU Workstation (Host), prepare the `uv` environment while online (recommended for CUDA wheels) and then run completely offline on campus/hotspot LAN:

```powershell
# Optional: pre-warm CUDA PyTorch wheel in uv cache before disconnecting WAN
uv sync

# Start headless Host in pure LAN mode (no internet relay required)
relay-daemon --repo-root . serve --lan-only
```

3. Open **SyncEverything** Desktop on your laptop, open your local copy of `pytorch-cuda-demo`, paste the pairing ticket in **Pair Device**, and dispatch `train`. When you commit a hyperparameter or code edit on the laptop, SyncEverything pushes the Git bundle to the Host and automatically marks prior GPU `Result` nodes as `Outdated`.
