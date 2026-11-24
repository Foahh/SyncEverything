#!/usr/bin/env python3
"""Generate the 4 publication-grade Track 5 figures (PNG 300 DPI + SVG + PDF).

Usage (zero modifications to project pyproject.toml or Cargo.toml):
    uv run --with matplotlib --with numpy python bench/results/plot_figures.py
"""

from __future__ import annotations

import csv
import json
import statistics
import sys
from pathlib import Path
from typing import Any

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
import numpy as np


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "eval"))
from model_runs import load_model_runs

BENCH_RESULTS = REPO_ROOT / "bench" / "results"
CHAOS_RESULTS = REPO_ROOT / "bench" / "chaos-cases" / "results"
MODEL_RESULTS = REPO_ROOT / "eval" / "models" / "results"
FIGURES_DIR = BENCH_RESULTS / "figures"

# Okabe-Ito colorblind-safe palette
OKABE = {
    "blue": "#0072B2",
    "orange": "#E69F00",
    "green": "#009E73",
    "vermilion": "#D55E00",
    "purple": "#CC79A7",
    "sky": "#56B4E9",
    "gray": "#595959",
    "light_gray": "#D9D9D9",
}


def setup_rcparams() -> None:
    plt.rcParams.update(
        {
            "figure.facecolor": "white",
            "axes.facecolor": "white",
            "axes.edgecolor": "#444444",
            "axes.linewidth": 0.85,
            "axes.spines.top": False,
            "axes.spines.right": False,
            "axes.grid": True,
            "axes.grid.axis": "y",
            "grid.color": "#D8D8D8",
            "grid.linestyle": "--",
            "grid.linewidth": 0.65,
            "axes.axisbelow": True,
            "font.family": "sans-serif",
            "font.sans-serif": ["DejaVu Sans", "Arial", "Helvetica"],
            "font.size": 9.5,
            "axes.titlesize": 11.2,
            "axes.titleweight": "bold",
            "axes.labelsize": 10.0,
            "xtick.labelsize": 9.0,
            "ytick.labelsize": 9.0,
            "legend.fontsize": 8.6,
            "legend.frameon": True,
            "legend.facecolor": "white",
            "legend.edgecolor": "#CCCCCC",
        }
    )


def save_figure(fig: plt.Figure, stem: str) -> None:
    FIGURES_DIR.mkdir(parents=True, exist_ok=True)
    for ext in ("png", "svg", "pdf"):
        out = FIGURES_DIR / f"{stem}.{ext}"
        kwargs: dict[str, Any] = {"bbox_inches": "tight", "pad_inches": 0.14}
        if ext == "png":
            kwargs["dpi"] = 300
        fig.savefig(out, **kwargs)
    plt.close(fig)
    print(f"[plot] Saved {stem}.{{png,svg,pdf}}", flush=True)


def plot_fig1_baseline_comparison() -> None:
    summary_path = BENCH_RESULTS / "baseline_comparison_summary.json"
    runs_path = BENCH_RESULTS / "baseline_comparison_runs.jsonl"
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    runs = [json.loads(line) for line in runs_path.read_text(encoding="utf-8").splitlines() if line.strip()]

    baseline_order = ["file_sync_only", "static_template", "relay_v1"]
    baseline_labels = {
        "file_sync_only": "File / Git\nSync Only",
        "static_template": "Static\nTemplate",
        "relay_v1": "Relay v1.0\n(DAG + Worktree)",
    }
    colors = {
        "file_sync_only": OKABE["vermilion"],
        "static_template": OKABE["orange"],
        "relay_v1": OKABE["green"],
    }
    by_b = {b["baseline"]: b for b in summary["baselines"]}

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12.8, 5.0), gridspec_kw={"width_ratios": [1.05, 1.25]})
    fig.subplots_adjust(wspace=0.26)

    # Panel (a): First-Pass Autonomous vs Final Completion Rate + Runtime/Token Callouts
    x = np.arange(len(baseline_order))
    bw = 0.30
    gap = 0.04
    first_pass = [by_b[b]["first_pass_success_pct"] for b in baseline_order]
    final_pass = [by_b[b]["final_success_pct"] for b in baseline_order]

    b_first = ax1.bar(
        x - (bw + gap) / 2,
        first_pass,
        width=bw,
        color=[colors[b] for b in baseline_order],
        edgecolor="#222222",
        linewidth=0.7,
        alpha=0.95,
        label="First-Pass Autonomous (%)",
    )
    b_final = ax1.bar(
        x + (bw + gap) / 2,
        final_pass,
        width=bw,
        color=[colors[b] for b in baseline_order],
        edgecolor="#222222",
        linewidth=0.7,
        alpha=0.40,
        hatch="///",
        label="Final Completion (after Oracle)",
    )
    for idx, (r1, r2, b) in enumerate(zip(b_first, b_final, baseline_order)):
        ax1.text(
            r1.get_x() + r1.get_width() / 2.0,
            r1.get_height() + 1.8,
            f"{first_pass[idx]:.1f}%" if first_pass[idx] < 100.0 else "100%",
            ha="center",
            va="bottom",
            fontsize=8.1,
            fontweight="bold",
            color="#222222",
        )
        ax1.text(
            r2.get_x() + r2.get_width() / 2.0,
            r2.get_height() + 1.8,
            f"{final_pass[idx]:.1f}%" if final_pass[idx] < 100.0 else "100%",
            ha="center",
            va="bottom",
            fontsize=8.1,
            color="#333333",
        )
        wall_m = by_b[b]["wall_clock_mean_s"]
        wall_sd = by_b[b]["wall_clock_sd_s"]
        tok_k = by_b[b]["avg_total_tokens"] / 1000.0
        ax1.text(
            x[idx],
            34.0,
            f"Wall: {wall_m:.1f}±{wall_sd:.1f}s\n{tok_k:.1f}k tok/run",
            ha="center",
            va="center",
            fontsize=7.6,
            bbox={"boxstyle": "round,pad=0.25", "facecolor": "white", "edgecolor": "#555555", "alpha": 0.94, "lw": 0.65},
        )

    ax1.set_ylim(0, 122)
    ax1.set_xticks(x)
    ax1.set_xticklabels([baseline_labels[b] for b in baseline_order])
    ax1.set_ylabel("Task Completion Rate (%)")
    ax1.set_title("(a) Autonomous First-Pass vs. Final Completion")
    p_first = mpatches.Patch(facecolor="#666666", alpha=0.92, edgecolor="#222222", label="First-Pass Autonomous (%)")
    p_final = mpatches.Patch(facecolor="#666666", alpha=0.35, hatch="///", edgecolor="#222222", label="Final Completion (w/ Oracle)")
    ax1.legend(handles=[p_first, p_final], loc="upper left", fontsize=8.0)

    # Panel (b): Total Manual Corrections + Unrecovered Aborts Across B1-B4 (N=3 trials/task)
    task_order = [
        ("B1_decision_carryover", "B1: Decision\nCarryover"),
        ("B2_stale_metric_trap", "B2: Stale\nMetric Trap"),
        ("B3_missing_dep_heal", "B3: Missing Dep\n(Offline)"),
        ("B4_defect_dirty_tree", "B4: NameError\n+ Dirty WIP"),
    ]
    tx = np.arange(len(task_order))
    bw2 = 0.25
    offsets = {"file_sync_only": -bw2, "static_template": 0.0, "relay_v1": bw2}
    short_names = {
        "file_sync_only": "File / Git Sync Only",
        "static_template": "Static Template",
        "relay_v1": "Relay v1.0",
    }

    counts: dict[tuple[str, str], tuple[int, int]] = {}
    for b in baseline_order:
        for tid, _ in task_order:
            sub = [r for r in runs if r["baseline"] == b and r["task_id"] == tid]
            corr = sum(int(r["manual_supplements"]) for r in sub)
            aborts = sum(1 for r in sub if r["loop_aborted"])
            counts[(b, tid)] = (corr, aborts)

    for b in baseline_order:
        corrs = [counts[(b, tid)][0] for tid, _ in task_order]
        aborts = [counts[(b, tid)][1] for tid, _ in task_order]
        totals = [c + a for c, a in zip(corrs, aborts)]
        ax2.bar(
            tx + offsets[b],
            corrs,
            width=bw2,
            color=colors[b],
            edgecolor="#222222",
            linewidth=0.65,
            label=f"{short_names[b]} ({sum(corrs)} inputs, {sum(aborts)} aborts)",
        )
        ax2.bar(
            tx + offsets[b],
            aborts,
            bottom=corrs,
            width=bw2,
            color=colors[b],
            alpha=0.35,
            hatch="xxx",
            edgecolor="#222222",
            linewidth=0.65,
        )
        for idx, (c, a, tot) in enumerate(zip(corrs, aborts, totals)):
            xpos = tx[idx] + offsets[b]
            if tot == 0:
                ax2.text(
                    xpos,
                    0.06,
                    "0",
                    ha="center",
                    va="bottom",
                    fontsize=7.9,
                    fontweight="bold" if b == "relay_v1" else "normal",
                    color=OKABE["green"] if b == "relay_v1" else "#555555",
                )
            else:
                if a == 0:
                    tag = f"{c}c"
                elif c == 0:
                    tag = f"{a}a"
                else:
                    tag = f"{c}c+{a}a"
                ax2.text(xpos, tot + 0.08, tag, ha="center", va="bottom", fontsize=7.8, fontweight="bold", color="#222222")

    ax2.set_ylim(0, max(3, max(sum(pair) for pair in counts.values())) * 1.3)
    ax2.yaxis.set_major_locator(plt.MaxNLocator(integer=True))
    ax2.set_xticks(tx)
    ax2.set_xticklabels([lbl for _, lbl in task_order])
    ax2.set_ylabel("Events per Task (Count)")
    ax2.set_title(f"(b) Human Inputs & Aborts per Task (N={summary['trials_per_task']} Trials)")
    ax2.text(
        0.98,
        0.64,
        "c = clarification or correction\na = aborted run",
        transform=ax2.transAxes,
        ha="right",
        va="top",
        fontsize=7.5,
        bbox={"boxstyle": "round,pad=0.3", "facecolor": "#F8FAFC", "edgecolor": "#CBD5E1", "lw": 0.7},
    )
    ax2.legend(loc="upper right", fontsize=7.8)

    n_total = len(runs)
    fig.suptitle(
        f"Controlled Three-Baseline Continuation Comparison (Model: {summary['model']}, {n_total} Live Runs)",
        fontsize=12.6,
        fontweight="bold",
        y=1.02,
    )
    save_figure(fig, "fig1_baseline_comparison")


def plot_fig2_context_ablation() -> None:
    scaling_path = BENCH_RESULTS / "context_ablation_scaling.csv"
    summary_path = BENCH_RESULTS / "context_ablation_summary.json"
    summary = json.loads(summary_path.read_text(encoding="utf-8"))

    with scaling_path.open("r", encoding="utf-8") as fh:
        scaling_rows = list(csv.DictReader(fh))
    hashes = {(r["snapshot_id"], r["strategy"]): r["context_bundle_hash"] for r in scaling_rows}
    for line in (BENCH_RESULTS / "context_ablation_runs.jsonl").read_text(encoding="utf-8").splitlines():
        if line.strip():
            row = json.loads(line)
            if row["context_bundle_hash"] != hashes[(row["snapshot_id"], row["strategy"])]:
                raise ValueError("Figure 2 requires measurements and scaling rows from identical context bundles")

    strat_order = ["full_transcript", "flat_summary", "dag_bundle"]
    strat_labels = {
        "full_transcript": "Full Transcript",
        "flat_summary": "Flat Summary",
        "dag_bundle": "DAG Bundle (Relay)",
    }
    strat_colors = {
        "full_transcript": OKABE["vermilion"],
        "flat_summary": OKABE["orange"],
        "dag_bundle": OKABE["green"],
    }
    strat_markers = {"full_transcript": "o", "flat_summary": "s", "dag_bundle": "^"}

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12.6, 4.9), gridspec_kw={"width_ratios": [1.0, 1.18]})
    fig.subplots_adjust(wspace=0.26)

    # Panel (a): Token Scaling across Depth 1, 3, 6, 10
    depth_ids = ["depth_1", "depth_3", "depth_6", "depth_10"]
    depth_lookup = {(r["snapshot_id"], r["strategy"]): int(r["estimated_tokens"]) for r in scaling_rows}
    depths = [1, 3, 6, 10]

    for strat in strat_order:
        ys = [depth_lookup[(did, strat)] for did in depth_ids]
        ax1.plot(
            depths,
            ys,
            marker=strat_markers[strat],
            markersize=6.5,
            linewidth=2.1,
            color=strat_colors[strat],
            label=strat_labels[strat],
        )
        for dx, dy in zip(depths, ys):
            if strat == "flat_summary" and dx == 1:
                ox, oy, ha, va = (12, -2, "left", "top")
            elif strat == "dag_bundle" and dx == 1:
                ox, oy, ha, va = (10, 6, "left", "bottom")
            elif strat == "flat_summary":
                ox, oy, ha, va = (0, -10, "center", "top")
            else:
                ox, oy, ha, va = (0, 7, "center", "bottom")
            ax1.annotate(
                f"{dy}",
                xy=(dx, dy),
                xytext=(ox, oy),
                textcoords="offset points",
                ha=ha,
                va=va,
                fontsize=7.8,
                color="#222222",
            )

    ft10 = depth_lookup[("depth_10", "full_transcript")]
    db10 = depth_lookup[("depth_10", "dag_bundle")]
    reduction = 100.0 * (1.0 - db10 / ft10) if ft10 else 0.0
    ax1.annotate(
        f"-{reduction:.1f}% tokens\n& 0 stale leaks\nat depth 10",
        xy=(10, db10),
        xytext=(6.6, db10 + (ft10 - db10) * 0.34),
        arrowprops={"arrowstyle": "->", "color": OKABE["green"], "lw": 1.2},
        fontsize=8.3,
        fontweight="bold",
        color=OKABE["green"],
        bbox={"boxstyle": "round,pad=0.3", "facecolor": "#F2FBF7", "edgecolor": OKABE["green"], "lw": 0.8},
    )

    ax1.set_xticks(depths)
    ax1.set_xlabel("Experiment History Depth (Sequential Commits & Evidence Nodes)")
    ax1.set_ylabel("Deterministic Estimated Context Tokens")
    ax1.set_ylim(-250, ft10 * 1.18)
    ax1.set_xlim(0.4, 10.6)
    ax1.set_title("(a) Context Size Scaling (compose-context)")
    ax1.legend(loc="upper left")

    # Panel (b): Empirical Stale Hallucination Rate (%) by Model & Strategy
    matrix = summary["matrix"]
    if any(row.get("n_errors", 0) for row in matrix):
        raise ValueError("Resolve the failed context-ablation requests with --retry-errors before plotting")
    models_seen: list[str] = []
    for row in matrix:
        if row["model"] not in models_seen:
            models_seen.append(row["model"])
    mx = np.arange(len(models_seen))
    bw = 0.25
    offsets = {"full_transcript": -bw, "flat_summary": 0.0, "dag_bundle": bw}
    mat_lookup = {(r["model"], r["strategy"]): r for r in matrix}

    for strat in strat_order:
        vals = [mat_lookup[(m, strat)]["stale_hallucination_rate_pct"] for m in models_seen]
        bars = ax2.bar(
            mx + offsets[strat],
            vals,
            width=bw,
            color=strat_colors[strat],
            edgecolor="#222222",
            linewidth=0.65,
            label=strat_labels[strat],
        )
        for rect, val in zip(bars, vals):
            ax2.text(
                rect.get_x() + rect.get_width() / 2.0,
                val + 1.8,
                f"{val:.1f}%",
                ha="center",
                va="bottom",
                fontsize=7.9,
                fontweight="bold" if strat == "dag_bundle" else "normal",
                color=OKABE["green"] if (strat == "dag_bundle" and val == 0.0) else "#222222",
            )

    ax2.set_ylim(0, 118)
    ax2.set_xticks(mx)
    ax2.set_xticklabels(models_seen)
    ax2.set_ylabel("Stale-Evidence Hallucination Rate (%)")
    ax2.set_title("(b) Empirical Hallucination Rate Across Models (Q1–Q6)")
    ax2.legend(loc="upper left")

    fig.suptitle(
        "Context Representation Ablation: Deterministic Scaling & Physical STALE Quarantine",
        fontsize=12.6,
        fontweight="bold",
        y=1.02,
    )
    save_figure(fig, "fig2_context_ablation")


def plot_fig3_model_pareto() -> None:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for d in load_model_runs(MODEL_RESULTS):
        grouped.setdefault(str(d["model"]), []).append(d)
    if not grouped:
        raise ValueError("Figure 3 requires live MCP evaluation results")

    order = [
        "qwen3.5:4b (no-think)",
        "qwen3.5:9b (no-think)",
        "qwen3.5:4b",
        "qwen3.5:9b",
        "deepseek-flash",
    ]
    present = [m for m in order if m in grouped] + [m for m in grouped if m not in order]

    palette = {
        "deepseek-flash": OKABE["green"],
        "qwen3.5:4b (no-think)": OKABE["sky"],
        "qwen3.5:9b (no-think)": OKABE["blue"],
        "qwen3.5:4b": OKABE["orange"],
        "qwen3.5:9b": OKABE["purple"],
    }
    markers = {
        "deepseek-flash": "*",
        "qwen3.5:4b (no-think)": "o",
        "qwen3.5:9b (no-think)": "s",
        "qwen3.5:4b": "D",
        "qwen3.5:9b": "^",
    }

    for model in present:
        markers.setdefault(model, "o")
        palette.setdefault(model, "#555555")

    stats_rows: list[dict[str, Any]] = []
    for m in present:
        items = grouped[m]
        lat = [float(x["summary"]["median_latency_ms"]) / 1000.0 for x in items]
        pr = [float(x["summary"]["pass_rate_pct"]) for x in items]
        rec = [float(x["summary"]["avg_tool_recall"]) * 100.0 for x in items]
        stale = [float(x["summary"]["stale_hallucination_rate_pct"]) for x in items]
        tok = [float(x["summary"]["total_tokens"]) for x in items]
        stats_rows.append(
            {
                "model": m,
                "n": len(items),
                "lat_m": statistics.mean(lat),
                "lat_sd": statistics.stdev(lat) if len(lat) >= 2 else 0.0,
                "pass_m": statistics.mean(pr),
                "pass_sd": statistics.stdev(pr) if len(pr) >= 2 else 0.0,
                "rec_m": statistics.mean(rec),
                "rec_sd": statistics.stdev(rec) if len(rec) >= 2 else 0.0,
                "stale_m": statistics.mean(stale),
                "tok_m": statistics.mean(tok) / 1000.0,
            }
        )

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12.8, 5.2))
    fig.subplots_adjust(wspace=0.25, bottom=0.19)

    # Custom explicit data coordinates + leader arrows to avoid any box collisions
    pos_p1 = {
        "deepseek-flash": (18.5, 97.0, "left"),
        "qwen3.5:4b (no-think)": (11.0, 29.0, "center"),
        "qwen3.5:9b (no-think)": (28.5, 29.0, "center"),
        "qwen3.5:4b": (24.5, 79.0, "center"),
        "qwen3.5:9b": (37.5, 67.5, "center"),
    }

    for row in stats_rows:
        m = row["model"]
        ms = 14 if markers[m] == "*" else 9
        ax1.errorbar(
            row["lat_m"],
            row["pass_m"],
            xerr=row["lat_sd"],
            yerr=row["pass_sd"],
            fmt=markers[m],
            markersize=ms,
            color=palette[m],
            markeredgecolor="#222222",
            markeredgewidth=0.75,
            ecolor=palette[m],
            elinewidth=1.2,
            capsize=3.5,
            label=f"{m} (N={row['n']})",
            zorder=4,
        )
        tx, ty, ha = pos_p1.get(m, (row["lat_m"] + 3, row["pass_m"] + 8, "left"))
        ax1.annotate(
            f"{m}\n{row['pass_m']:.1f}% | {row['tok_m']:.0f}k tok",
            xy=(row["lat_m"], row["pass_m"]),
            xycoords="data",
            xytext=(tx, ty),
            textcoords="data",
            ha=ha,
            va="center",
            fontsize=7.7,
            arrowprops={"arrowstyle": "->", "color": palette[m], "lw": 0.9, "shrinkA": 2, "shrinkB": 5},
            bbox={"boxstyle": "round,pad=0.22", "facecolor": "white", "edgecolor": palette[m], "alpha": 0.94, "lw": 0.75},
            zorder=5,
        )

    ax1.grid(True, which="both", axis="both", linestyle="--", alpha=0.35)
    ax1.set_xlim(0, max(r["lat_m"] + r["lat_sd"] for r in stats_rows) * 1.16)
    ax1.set_ylim(0, 108)
    ax1.set_xlabel("Median Scenario LLM Latency (s, Mean ± SD)")
    ax1.set_ylabel("Scenario Pass Rate (%, Mean ± SD)")
    ax1.set_title("(a) Latency vs. Task Success")

    # Panel (b): Tool Recall vs Latency + Stale Leakage Indicator
    pos_p2 = {
        "deepseek-flash": (10.5, 102.8, "left"),
        "qwen3.5:4b (no-think)": (9.5, 81.8, "center"),
        "qwen3.5:9b (no-think)": (23.5, 86.2, "center"),
        "qwen3.5:4b": (36.5, 98.0, "center"),
        "qwen3.5:9b": (38.0, 81.8, "center"),
    }
    for row in stats_rows:
        m = row["model"]
        ms = 14 if markers[m] == "*" else 9
        ax2.errorbar(
            row["lat_m"],
            row["rec_m"],
            xerr=row["lat_sd"],
            yerr=row["rec_sd"],
            fmt=markers[m],
            markersize=ms,
            color=palette[m],
            markeredgecolor="#222222",
            markeredgewidth=0.75,
            ecolor=palette[m],
            elinewidth=1.2,
            capsize=3.5,
            zorder=4,
        )
        tx, ty, ha = pos_p2.get(m, (row["lat_m"] + 3, row["rec_m"] - 5, "left"))
        ax2.annotate(
            f"{m}\nRecall {row['rec_m']:.1f}% | Stale {row['stale_m']:.1f}%",
            xy=(row["lat_m"], row["rec_m"]),
            xycoords="data",
            xytext=(tx, ty),
            textcoords="data",
            ha=ha,
            va="center",
            fontsize=7.7,
            arrowprops={"arrowstyle": "->", "color": palette[m], "lw": 0.9, "shrinkA": 2, "shrinkB": 5},
            bbox={"boxstyle": "round,pad=0.22", "facecolor": "white", "edgecolor": palette[m], "alpha": 0.94, "lw": 0.75},
            zorder=5,
        )

    ax2.grid(True, which="both", axis="both", linestyle="--", alpha=0.35)
    ax2.set_xlim(0, max(r["lat_m"] + r["lat_sd"] for r in stats_rows) * 1.16)
    ax2.set_ylim(0, 106)
    ax2.set_xlabel("Median Scenario LLM Latency (s, Mean ± SD)")
    ax2.set_ylabel("Average MCP Tool Recall (%, Mean ± SD)")
    ax2.set_title("(b) Latency vs. Tool Recall & Stale Leakage")

    handles, labels = ax1.get_legend_handles_labels()
    fig.legend(handles, labels, loc="lower center", ncol=len(labels), fontsize=8.1, bbox_to_anchor=(0.5, 0.01))

    counts = sorted({row["n"] for row in stats_rows})
    n_runs_text = f"N={counts[0]}" if len(counts) == 1 else f"N={counts[0]}–{counts[-1]}"
    fig.suptitle(
        f"Local Quantized Models (Ollama 4B/9B) vs. Cloud API Pareto Tradeoff ({n_runs_text} Independent Trials)",
        fontsize=12.4,
        fontweight="bold",
        y=0.98,
    )
    save_figure(fig, "fig3_model_pareto")


def plot_fig4_chaos_self_healing() -> None:
    runners = [
        ("Scripted\n(+P2P Gate)", json.loads((CHAOS_RESULTS / "chaos-report.json").read_text(encoding="utf-8"))),
        ("deepseek-flash\n(Cloud API)", json.loads((CHAOS_RESULTS / "chaos-llm-report.json").read_text(encoding="utf-8"))),
        ("qwen3.5:9b\n(Local 16K)", json.loads((CHAOS_RESULTS / "chaos-llm-qwen3.5_9b.json").read_text(encoding="utf-8"))),
        ("qwen3.5:4b\n(Local 16K)", json.loads((CHAOS_RESULTS / "chaos-llm-qwen3.5_4b.json").read_text(encoding="utf-8"))),
    ]

    case_labels = [
        ("chaos-01-missing-dep", "C01: Missing Dep (uv_spec)"),
        ("chaos-02-upstream-edit", "C02: Upstream Edit (STALE)"),
        ("chaos-03-same-line-conflict", "C03: Same-Line Conflict"),
        ("chaos-04-mid-run-disconnect", "C04: Mid-Run Disconnect"),
        ("chaos-05-runtime-exception", "C05: NameError (code_diff)"),
        ("chaos-06-corrupted-lock", "C06: Corrupted uv.lock"),
        ("chaos-07-stale-evidence-trap", "C07: Stale Evidence Trap"),
        ("chaos-08-duplicate-replay", "C08: Duplicate Task Replay"),
    ]

    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(13.2, 5.2), gridspec_kw={"width_ratios": [1.38, 1.0]})
    fig.subplots_adjust(wspace=0.25)

    # Panel (a): Outcome Matrix Heatmap (8 cases x 4 runners)
    n_rows = len(case_labels)
    n_cols = len(runners)
    ax1.set_xlim(-0.5, n_cols - 0.5)
    ax1.set_ylim(-0.5, n_rows - 0.5)
    ax1.invert_yaxis()
    ax1.grid(False)

    for col_idx, (r_label, data) in enumerate(runners):
        cases_by_id = {c["case_id"]: c for c in data["cases"]}
        for row_idx, (cid, _) in enumerate(case_labels):
            c = cases_by_id[cid]
            passed = bool(c["passed"])
            stale_leak = bool(c.get("stale_hallucination", False))
            retries = int(c.get("retries", 0))
            is_scripted = col_idx == 0

            if passed:
                bg = "#D6EFE5"
                fg = "#0B5D43"
                text = "PASS" if is_scripted else f"PASS (r={retries})"
            elif stale_leak:
                bg = "#FDE6C4"
                fg = "#8C4500"
                text = "STALE LEAK"
            else:
                bg = "#FAD4D0"
                fg = "#8F1D14"
                text = f"FAIL (r={retries})"

            rect = mpatches.Rectangle(
                (col_idx - 0.46, row_idx - 0.40),
                0.92,
                0.80,
                facecolor=bg,
                edgecolor="#888888",
                linewidth=0.7,
            )
            ax1.add_patch(rect)
            ax1.text(
                col_idx,
                row_idx,
                text,
                ha="center",
                va="center",
                fontsize=8.1,
                fontweight="bold",
                color=fg,
            )

    ax1.set_xticks(range(n_cols))
    ax1.set_xticklabels([r[0] for r in runners])
    ax1.set_yticks(range(n_rows))
    ax1.set_yticklabels([lbl for _, lbl in case_labels])
    ax1.set_title("(a) Fault-Injection Outcome Matrix (8/8 Chaos Cases)")
    ax1.spines["left"].set_visible(False)
    ax1.spines["bottom"].set_visible(False)
    ax1.tick_params(length=0)

    # Panel (b): Pass Rate, Self-Heal Rate, and Retry Breakdown across Live Models
    live_runners = runners[1:]
    lx = np.arange(len(live_runners))
    bw = 0.28
    overall_rates = [d["summary"]["pass_rate_pct"] for _, d in live_runners]
    heal_rates = [d["summary"]["self_heal_rate_pct"] for _, d in live_runners]

    # Retry counts by category on twin axis
    uv_retries = []
    diff_retries = []
    for _, d in live_runners:
        c_by_id = {c["case_id"]: c for c in d["cases"]}
        uv_retries.append(c_by_id["chaos-01-missing-dep"].get("retries", 0))
        diff_retries.append(
            c_by_id["chaos-03-same-line-conflict"].get("retries", 0)
            + c_by_id["chaos-05-runtime-exception"].get("retries", 0)
            + c_by_id["chaos-06-corrupted-lock"].get("retries", 0)
        )

    b1 = ax2.bar(lx - bw * 0.56, overall_rates, width=bw, color=OKABE["blue"], edgecolor="#222222", lw=0.65, label="Overall Pass Rate (%)")
    b2 = ax2.bar(lx + bw * 0.56, heal_rates, width=bw, color=OKABE["green"], edgecolor="#222222", lw=0.65, label="Self-Heal Rate (C01,C03,C05,C06)")
    for bars in (b1, b2):
        for rect in bars:
            h = rect.get_height()
            label_txt = "100%" if h == 100.0 else f"{h:.1f}%"
            ax2.text(rect.get_x() + rect.get_width() / 2.0, h + 1.8, label_txt, ha="center", va="bottom", fontsize=7.9)

    ax2.set_ylim(0, 126)
    ax2.set_xticks(lx)
    ax2.set_xticklabels([lbl.replace("\n", "\n") for lbl, _ in live_runners])
    ax2.set_ylabel("Success Rate (%)")
    ax2.set_title("(b) Self-Healing Rate & Retry Breakdown")

    ax2_r = ax2.twinx()
    ax2_r.spines["top"].set_visible(False)
    ax2_r.grid(False)
    total_retries = [u + df for u, df in zip(uv_retries, diff_retries)]
    ax2_r.plot(
        lx,
        total_retries,
        color=OKABE["vermilion"],
        marker="D",
        markersize=7.0,
        linewidth=2.0,
        label="Total Patch Retries (Right Axis)",
        zorder=5,
    )
    retry_offsets = [(24, 20), (0, 14), (4, 18)]
    for rx, rt, ur, dr, off in zip(lx, total_retries, uv_retries, diff_retries, retry_offsets):
        ax2_r.annotate(
            f"{rt} retries\n(uv={ur}, diff={dr})",
            xy=(rx, rt),
            xytext=off,
            textcoords="offset points",
            ha="center",
            fontsize=7.5,
            color=OKABE["vermilion"],
            fontweight="bold",
            bbox={"boxstyle": "round,pad=0.2", "facecolor": "white", "edgecolor": OKABE["vermilion"], "alpha": 0.96, "lw": 0.65},
            arrowprops={"arrowstyle": "-", "color": OKABE["vermilion"], "lw": 0.8},
            zorder=10,
        )
    ax2_r.set_ylim(-1.2, max(total_retries + [10]) * 1.40)
    ax2_r.set_ylabel("Total Patch Retries (Count)", color=OKABE["vermilion"])
    ax2_r.tick_params(axis="y", colors=OKABE["vermilion"])

    h1, l1 = ax2.get_legend_handles_labels()
    h2, l2 = ax2_r.get_legend_handles_labels()
    ax2.legend(h1 + h2, l1 + l2, loc="upper left", fontsize=7.6)

    fig.suptitle(
        "Chaos Suite Fault-Injection Evaluation: 8 Fault Classes & Self-Healing Retries",
        fontsize=12.6,
        fontweight="bold",
        y=1.02,
    )
    save_figure(fig, "fig4_chaos_self_healing")


def main() -> int:
    setup_rcparams()
    plot_fig1_baseline_comparison()
    plot_fig2_context_ablation()
    plot_fig3_model_pareto()
    plot_fig4_chaos_self_healing()
    print("[plot] All 4 publication figures generated cleanly.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
