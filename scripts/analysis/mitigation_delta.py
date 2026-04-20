#!/usr/bin/env python3
"""Mitigation delta analysis — solver localization skill vs topology vulnerability.

mitigation_delta = cascade_spread - structural_cascade
- Negative: solver localizes fault impact (replans around dead cell)
- Positive: solver propagates beyond topological vulnerability (rare)

Paradigm-level comparison: does decentralized (TP) localize as well as
centralized (PIBT/RHCR) at matched topology?

Outputs:
- `results/aisle_width/analysis/mitigation_delta_by_aisle.png`
- `results/aisle_width/analysis/mitigation_delta_by_solver.png`
- `results/aisle_width/analysis/mitigation_delta_stats.json`
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

OUT = Path("results/aisle_width")
SRC = OUT / "merged" if (OUT / "merged").exists() else OUT
ANA = OUT / "analysis"
ANA.mkdir(parents=True, exist_ok=True)

AISLE = {
    "warehouse_single_dock": 1,
    "warehouse_sd_w2": 2,
    "warehouse_sd_w3": 3,
}
TIER = {20: "L", 40: "M", 60: "H", 36: "L", 72: "M", 108: "H", 50: "L", 100: "M", 151: "H"}
COLORS = {"pibt": "#1f77b4", "rhcr_pbs": "#ff7f0e", "token_passing": "#2ca02c"}
LABELS = {"pibt": "PIBT", "rhcr_pbs": "RHCR-PBS", "token_passing": "Token Passing"}


def load() -> pd.DataFrame:
    frames = [pd.read_csv(f) for f in sorted(SRC.glob("*_summary.csv"))]
    df = pd.concat(frames, ignore_index=True)
    df["aisle"] = df["topology"].map(AISLE)
    df["tier"] = df["num_agents"].map(TIER)
    return df


def main():
    df = load()
    df_agg = df.groupby(["solver", "aisle", "tier"], as_index=False).agg(
        delta=("mitigation_delta_mean", "mean"),
        delta_std=("mitigation_delta_std", "mean"),
        solver_cascade=("cascade_spread_mean", "mean"),
        structural=("structural_cascade_mean", "mean"),
    )

    # ── Line plot: mitigation delta vs aisle, one line per solver, faceted by tier ──
    fig, axes = plt.subplots(1, 3, figsize=(12, 4), sharey=True)
    for ax, tier in zip(axes, ["L", "M", "H"]):
        for solver in ["pibt", "rhcr_pbs", "token_passing"]:
            sub = df_agg[(df_agg["solver"] == solver) & (df_agg["tier"] == tier)]
            if sub.empty:
                continue
            sub = sub.sort_values("aisle")
            ax.plot(
                sub["aisle"], sub["delta"],
                "o-", color=COLORS[solver], label=LABELS[solver], linewidth=2, markersize=8,
            )
        ax.axhline(0, color="grey", linestyle=":", linewidth=0.8)
        ax.set_xlabel("Aisle width")
        ax.set_xticks([1, 2, 3])
        ax.set_title(f"Tier {tier}", fontsize=10)
        ax.grid(True, alpha=0.3)
    axes[0].set_ylabel("Mitigation Δ (solver − structural)")
    axes[0].legend(fontsize=9, loc="lower left")
    fig.suptitle(
        "Mitigation Δ by aisle width — negative = solver localizes below topology prediction",
        fontsize=11,
    )
    fig.tight_layout()
    fig.savefig(ANA / "mitigation_delta_by_aisle.png", dpi=150)
    plt.close(fig)

    # ── Bar chart: aggregate solver mitigation at each aisle ─────────
    fig, ax = plt.subplots(figsize=(8, 4))
    solvers = ["pibt", "rhcr_pbs", "token_passing"]
    aisles = [1, 2, 3]
    x = np.arange(len(aisles))
    width = 0.25
    for i, solver in enumerate(solvers):
        ys = []
        errs = []
        for a in aisles:
            sub = df_agg[(df_agg["solver"] == solver) & (df_agg["aisle"] == a)]
            ys.append(sub["delta"].mean() if not sub.empty else np.nan)
            errs.append(sub["delta"].std() if not sub.empty else 0)
        ax.bar(x + i * width, ys, width, yerr=errs,
               label=LABELS[solver], color=COLORS[solver], alpha=0.85, capsize=3)
    ax.axhline(0, color="grey", linestyle=":", linewidth=0.8)
    ax.set_xticks(x + width)
    ax.set_xticklabels([f"w{a}" for a in aisles])
    ax.set_xlabel("Aisle width")
    ax.set_ylabel("Mitigation Δ (avg across tiers)")
    ax.set_title("Solver mitigation skill grows with bypass capacity", fontsize=11)
    ax.legend(fontsize=9)
    ax.grid(True, alpha=0.3, axis="y")
    fig.tight_layout()
    fig.savefig(ANA / "mitigation_delta_by_solver.png", dpi=150)
    plt.close(fig)

    # ── Stats JSON ────────────────────────────────────────────────────
    stats = {
        "mean_delta_by_solver_aisle": {
            f"{s}_w{a}": float(
                df_agg[(df_agg["solver"] == s) & (df_agg["aisle"] == a)]["delta"].mean()
            )
            for s in solvers for a in aisles
        },
        "divergence_per_aisle_unit": {
            s: float(
                (df_agg[(df_agg["solver"] == s) & (df_agg["aisle"] == 3)]["delta"].mean()
                 - df_agg[(df_agg["solver"] == s) & (df_agg["aisle"] == 1)]["delta"].mean()) / 2
            )
            for s in solvers
        },
    }
    with open(ANA / "mitigation_delta_stats.json", "w") as f:
        json.dump(stats, f, indent=2)

    print("Mean mitigation Δ (solver × aisle):")
    for s in solvers:
        row = [
            stats['mean_delta_by_solver_aisle'][f'{s}_w{a}'] for a in aisles
        ]
        print(f"  {LABELS[s]:<15} w1={row[0]:.2f}  w2={row[1]:.2f}  w3={row[2]:.2f}")

    print("\nDivergence rate (Δ units per aisle step):")
    for s in solvers:
        v = stats["divergence_per_aisle_unit"][s]
        print(f"  {LABELS[s]:<15} {v:.3f}")

    print(f"\nWrote: {ANA}/mitigation_delta_by_aisle.png")
    print(f"Wrote: {ANA}/mitigation_delta_by_solver.png")
    print(f"Wrote: {ANA}/mitigation_delta_stats.json")


if __name__ == "__main__":
    main()
