"""Lightweight seeded PyTorch training/evaluation demo for SyncEverything.

Trains a 3-layer GELU MLP on a synthetic 4-class spiral dataset (no external downloads).
Automatically picks CUDA -> MPS -> CPU unless overridden with `--device`, and prints a
single JSON metrics object to stdout for DAG Evidence recording.
"""

from __future__ import annotations

import argparse
import json
import math
import time
from typing import Tuple

import torch
import torch.nn as nn
import torch.nn.functional as F
from torch.utils.data import DataLoader, TensorDataset


def select_device(requested: str) -> torch.device:
    if requested == "cuda":
        if not torch.cuda.is_available():
            raise RuntimeError("CUDA requested via --device cuda, but no CUDA device is available.")
        return torch.device("cuda")
    if requested == "mps":
        mps_backend = getattr(torch.backends, "mps", None)
        if not mps_backend or not mps_backend.is_available():
            raise RuntimeError("MPS requested via --device mps, but MPS is not available.")
        return torch.device("mps")
    if requested == "cpu":
        return torch.device("cpu")

    if torch.cuda.is_available():
        return torch.device("cuda")
    mps_backend = getattr(torch.backends, "mps", None)
    if mps_backend and mps_backend.is_available():
        return torch.device("mps")
    return torch.device("cpu")


def describe_device(device: torch.device) -> str:
    if device.type == "cuda":
        return torch.cuda.get_device_name(device)
    if device.type == "mps":
        return "Apple Silicon MPS"
    return "CPU"


def sync_device(device: torch.device) -> None:
    if device.type == "cuda":
        torch.cuda.synchronize(device)
    elif device.type == "mps" and hasattr(torch, "mps"):
        torch.mps.synchronize()


def make_spiral_dataset(
    num_samples: int, num_classes: int = 4, seed: int = 42
) -> Tuple[torch.Tensor, torch.Tensor]:
    gen = torch.Generator().manual_seed(seed)
    per_class = max(num_samples // num_classes, 16)
    xs = []
    ys = []
    for cls_idx in range(num_classes):
        t = torch.linspace(0.15, 1.0, per_class)
        angle = t * 2.5 * math.pi + cls_idx * (2.0 * math.pi / num_classes)
        noise = torch.randn((per_class, 2), generator=gen) * 0.08
        coords = torch.stack([t * torch.sin(angle), t * torch.cos(angle)], dim=1) + noise
        xs.append(coords)
        ys.append(torch.full((per_class,), cls_idx, dtype=torch.long))

    features = torch.cat(xs, dim=0)
    labels = torch.cat(ys, dim=0)
    perm = torch.randperm(features.size(0), generator=gen)
    return features[perm], labels[perm]


class SpiralMlp(nn.Module):
    def __init__(self, hidden: int, num_classes: int = 4) -> None:
        super().__init__()
        self.net = nn.Sequential(
            nn.Linear(2, hidden),
            nn.GELU(),
            nn.Linear(hidden, hidden),
            nn.GELU(),
            nn.Linear(hidden, num_classes),
        )

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.net(x)


def run_smoke(device: torch.device, hidden: int, seed: int) -> dict[str, object]:
    torch.manual_seed(seed)
    features, labels = make_spiral_dataset(num_samples=64, seed=seed)
    model = SpiralMlp(hidden=hidden).to(device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=1e-2)

    sync_device(device)
    t0 = time.perf_counter()
    x = features.to(device)
    y = labels.to(device)
    logits = model(x)
    loss = F.cross_entropy(logits, y)
    optimizer.zero_grad(set_to_none=True)
    loss.backward()
    optimizer.step()
    sync_device(device)
    elapsed_ms = (time.perf_counter() - t0) * 1000.0

    loss_val = float(loss.item())
    if not math.isfinite(loss_val):
        raise RuntimeError(f"Smoke step produced non-finite loss: {loss_val}")

    return {
        "status": "ok",
        "device": device.type,
        "device_name": describe_device(device),
        "smoke_loss": round(loss_val, 4),
        "elapsed_ms": round(elapsed_ms, 2),
    }


def run_train(
    device: torch.device,
    epochs: int,
    lr: float,
    hidden: int,
    batch_size: int,
    samples: int,
    seed: int,
) -> dict[str, object]:
    torch.manual_seed(seed)
    features, labels = make_spiral_dataset(num_samples=samples, seed=seed)
    total = features.size(0)
    val_count = max(total // 5, 16)
    train_x, val_x = features[val_count:], features[:val_count]
    train_y, val_y = labels[val_count:], labels[:val_count]

    loader_gen = torch.Generator().manual_seed(seed)
    train_loader = DataLoader(
        TensorDataset(train_x, train_y),
        batch_size=min(batch_size, train_x.size(0)),
        shuffle=True,
        generator=loader_gen,
    )

    model = SpiralMlp(hidden=hidden).to(device)
    optimizer = torch.optim.AdamW(model.parameters(), lr=lr, weight_decay=1e-4)
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=max(epochs, 1))

    sync_device(device)
    t0 = time.perf_counter()

    final_train_loss = 0.0
    for _epoch in range(epochs):
        model.train()
        running_loss = 0.0
        seen = 0
        for bx, by in train_loader:
            bx = bx.to(device)
            by = by.to(device)
            optimizer.zero_grad(set_to_none=True)
            logits = model(bx)
            loss = F.cross_entropy(logits, by)
            loss.backward()
            optimizer.step()
            running_loss += float(loss.item()) * bx.size(0)
            seen += bx.size(0)
        scheduler.step()
        final_train_loss = running_loss / max(seen, 1)

    model.eval()
    with torch.no_grad():
        vx = val_x.to(device)
        vy = val_y.to(device)
        val_logits = model(vx)
        val_loss = float(F.cross_entropy(val_logits, vy).item())
        val_acc = float((val_logits.argmax(dim=1) == vy).float().mean().item())

    sync_device(device)
    elapsed_s = max(time.perf_counter() - t0, 1e-6)
    samples_per_sec = (train_x.size(0) * epochs) / elapsed_s

    return {
        "val_acc": round(val_acc, 4),
        "val_loss": round(val_loss, 4),
        "train_loss": round(final_train_loss, 4),
        "device": device.type,
        "device_name": describe_device(device),
        "samples_per_sec": round(samples_per_sec, 1),
        "epochs": epochs,
        "lr": lr,
        "hidden": hidden,
        "batch_size": batch_size,
        "samples": total,
        "seed": seed,
        "elapsed_ms": round(elapsed_s * 1000.0, 1),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Lightweight PyTorch CUDA/MPS/CPU demo")
    parser.add_argument("--smoke", action="store_true", help="Run fast 1-step preflight check")
    parser.add_argument("--epochs", type=int, default=40)
    parser.add_argument("--lr", type=float, default=0.005)
    parser.add_argument("--hidden", type=int, default=128)
    parser.add_argument("--batch_size", "--batch-size", dest="batch_size", type=int, default=256)
    parser.add_argument("--samples", type=int, default=4096)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--device", choices=["auto", "cuda", "mps", "cpu"], default="auto")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    device = select_device(args.device)
    metrics = (
        run_smoke(device=device, hidden=args.hidden, seed=args.seed)
        if args.smoke
        else run_train(
            device=device,
            epochs=args.epochs,
            lr=args.lr,
            hidden=args.hidden,
            batch_size=args.batch_size,
            samples=args.samples,
            seed=args.seed,
        )
    )
    print(json.dumps(metrics, indent=2))


if __name__ == "__main__":
    main()
