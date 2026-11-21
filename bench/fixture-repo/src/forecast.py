"""Deterministic rolling-window forecasting benchmark for Relay v1.0.

Runs cleanly with stdlib in <1s for smoke tests and sweeps window hyperparameters
for 3-group experiments while emitting machine-readable JSON metrics.
"""

from __future__ import annotations

import argparse
import json
import math
import time
from typing import Iterable


def generate_series(length: int = 240) -> list[float]:
    return [
        20.0
        + 4.5 * math.sin(2.0 * math.pi * t / 24.0)
        + 1.8 * math.cos(2.0 * math.pi * t / 7.0)
        + 0.03 * t
        for t in range(length)
    ]


def evaluate_window(series: list[float], window: int, alpha: float = 0.35) -> dict[str, float]:
    if window < 2 or window >= len(series):
        raise ValueError(f"Invalid window size: {window}")
    abs_errors: list[float] = []
    sq_errors: list[float] = []
    for idx in range(window, len(series)):
        hist = series[idx - window : idx]
        ma = sum(hist) / window
        ewma = hist[0]
        for val in hist[1:]:
            ewma = alpha * val + (1.0 - alpha) * ewma
        pred = 0.5 * ma + 0.5 * ewma
        err = series[idx] - pred
        abs_errors.append(abs(err))
        sq_errors.append(err * err)

    mae = sum(abs_errors) / len(abs_errors)
    rmse = math.sqrt(sum(sq_errors) / len(sq_errors))
    return {
        "window": float(window),
        "mae": round(mae, 5),
        "rmse": round(rmse, 5),
    }


def run_sweep(windows: Iterable[int], alpha: float, smoke: bool) -> dict[str, object]:
    t0 = time.perf_counter()
    series = generate_series(80 if smoke else 360)
    results = [evaluate_window(series, w, alpha=alpha) for w in windows]
    best = min(results, key=lambda r: r["mae"])
    elapsed_ms = round((time.perf_counter() - t0) * 1000.0, 2)
    return {
        "mode": "smoke" if smoke else "grid",
        "alpha": alpha,
        "elapsed_ms": elapsed_ms,
        "best_window": int(best["window"]),
        "best_mae": best["mae"],
        "runs": results,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Relay rolling-window forecast benchmark")
    parser.add_argument("--smoke", action="store_true", help="Run fast smoke preflight")
    parser.add_argument("--windows", default="7,14,28", help="Comma-separated window sizes")
    parser.add_argument("--alpha", type=float, default=0.35, help="EWMA smoothing factor")
    args = parser.parse_args()

    windows = [7] if args.smoke else [int(w.strip()) for w in args.windows.split(",") if w.strip()]
    report = run_sweep(windows=windows, alpha=args.alpha, smoke=args.smoke)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
