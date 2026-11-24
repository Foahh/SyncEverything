# PyTorch CUDA Demo (`SyncEverything` / `Relay`)

Lightweight, seeded PyTorch training and evaluation for demos over a direct LAN connection between a laptop and a workstation.

- Generates the 4-class spiral dataset in memory from a fixed `--seed`. Initial setup downloads the PyTorch dependencies.
- Automatic accelerator fallback: picks **CUDA -> Apple MPS -> CPU** unless overridden via `--device`.
- Emits structured JSON metrics (`val_acc`, `val_loss`, `device`, `device_name`, `samples_per_sec`, `elapsed_ms`) captured into the Decision-Evidence DAG.

## Presets (`relay.toml`)

| Preset | Purpose | Allowed Params |
|---|---|---|
| `smoke` | 1-step forward/backward & accelerator check | `device` |
| `train` | 40-epoch 3-layer GELU MLP training run | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |
| `eval` | Fast 15-epoch validation sweep | `epochs`, `lr`, `hidden`, `batch_size`, `samples`, `seed`, `device` |

## Quick Two-Machine LAN Setup

1. On the Host, copy the three project files to a new directory and initialize Git. While online, use `uv sync` to populate the local `uv` cache and verify the environment. For CUDA, use an index that provides PyTorch wheels for the Host's platform and CUDA version. [PyTorch installation options](https://pytorch.org/get-started/locally/)

```powershell
New-Item -ItemType Directory -Path ~/pytorch-cuda-demo
Copy-Item examples/pytorch-cuda-demo/train.py,examples/pytorch-cuda-demo/pyproject.toml,examples/pytorch-cuda-demo/relay.toml ~/pytorch-cuda-demo
Set-Location ~/pytorch-cuda-demo
git init -b main
git add train.py pyproject.toml relay.toml
git commit -m "init pytorch cuda demo"
uv sync
uv run --offline python train.py --smoke
```

> Note: `uv.lock` is intentionally omitted from the starter files so Windows CUDA workstations and Apple Silicon macOS laptops can each resolve their native PyTorch wheels out of the box. If you want strict lockfile pinning across homogeneous machines, commit your generated `uv.lock` and add `--locked` back to `relay.toml`.

2. Clone this same commit on the laptop. With both machines on the same LAN, start the daemon on the Host with the same uv cache and index settings used during setup. Relay creates a separate environment in each task worktree, so this cache supplies the dependencies when WAN access is unavailable. [uv lockfile and environments](https://docs.astral.sh/uv/concepts/projects/sync/)

```powershell
$env:UV_OFFLINE = "1"
relay-daemon --repo-root . serve --lan-only
```

3. Open **SyncEverything** Desktop on the laptop, open the demo repository, paste the pairing ticket in **Pair Device**, and dispatch `train`. To demonstrate invalidation, commit a change to `train.py` on the laptop. SyncEverything pushes the commit to the Host and marks the prior `Result` as `Outdated`.
