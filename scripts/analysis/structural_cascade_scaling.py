#!/usr/bin/env python3
"""Structural cascade scaling analysis.

Tests whether structural_cascade (solver-independent topological vulnerability)
scales with walkable area and aisle width at matched density.

Outputs:
- `results/aisle_width/analysis/structural_cascade_heatmap.png` — aisle × tier heatmap
- `results/aisle_width/analysis/structural_cascade_vs_walkable.png` — regression
- `results/aisle_width/analysis/structural_cascade_stats.json` — numerical results
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from scipy.stats import linregress

OUT = Path("results/aisle_width")
if (OUT / "merged_post_fix").exists():
    SRC = OUT / "merged_post_fix"
elif (OUT / "merged").exists():
    SRC = OUT / "merged"
else:
    SRC = OUT
ANA = OUT / "analysis"
ANA.mkdir(parents=True, exist_ok=True)

# Walkable cell counts (from topology JSON audit)
WALKABLE = {
    "warehouse_single_dock": 843,
    "warehouse_single_dock_w2": 1510,
    "warehouse_single_dock_w3": 2115,
}
AISLE = {
    "warehouse_single_dock": 1,
    "warehouse_single_dock_w2": 2,
    "warehouse_single_dock_w3": 3,
}
TIER = {20: "L", 40: "M", 60: "H", 36: "L", 72: "M", 108: "H", 50: "L", 100: "M", 151: "H"}


def load() -> pd.DataFrame:
    frames = [pd.read_csv(f) for f in sorted(SRC.glob("*_summary.csv"))]
    df = pd.concat(frames, ignore_index=True)
    df["aisle"] = df["topology"].map(AISLE)
    df["walkable"] = df["topology"].map(WALKABLE)
    df["tier"] = df["num_agents"].map(TIER)
    # Structural cascade is solver-independent by construction; average over
    # solvers to get a clean topology-only signal (runs with and without TP
    # contribute equally after averaging, so no bias from TP absence).
    return df


def main():
    df = load()

    # Aggregate across solvers + scenarios (structural_cascade is solver-independent)
    agg = df.groupby(["aisle", "tier", "num_agents", "walkable"], as_index=False).agg(
        structural=("structural_cascade_mean", "mean"),
        structural_std=("structural_cascade_std", "mean"),
    )

    # ── Heatmap ──────────────────────────────────────────────────────
    fig, ax = plt.subplots(figsize=(5.5, 3.5))
    pivot = agg.pivot(index="tier", columns="aisle", values="structural").reindex(["L", "M", "H"])
    im = ax.imshow(pivot.values, cmap="YlOrRd", aspect="auto")
    ax.set_xticks(range(pivot.shape[1]))
    ax.set_xticklabels([f"w{w}" for w in pivot.columns], fontsize=10)
    ax.set_yticks(range(pivot.shape[0]))
    ax.set_yticklabels(pivot.index, fontsize=10)
    ax.set_xlabel("Aisle width")
    ax.set_ylabel("Density tier")
    ax.set_title("Structural cascade (avg agents disrupted / fault)", fontsize=11)
    for i in range(pivot.shape[0]):
        for j in range(pivot.shape[1]):
            v = pivot.values[i, j]
            ax.text(
                j, i, f"{v:.2f}", ha="center", va="center",
                color="white" if v > pivot.values.max() * 0.55 else "black", fontsize=10,
            )
    plt.colorbar(im, ax=ax, shrink=0.8)
    fig.tight_layout()
    fig.savefig(ANA / "structural_cascade_heatmap.png", dpi=150)
    plt.close(fig)

    # ── Regression: structural_cascade vs walkable at matched density ───
    # For each tier, fit structural ~ walkable_area (linear regression)
    regressions = {}
    fig, axes = plt.subplots(1, 3, figsize=(12, 3.8), sharey=False)
    for ax, tier in zip(axes, ["L", "M", "H"]):
        sub = agg[agg["tier"] == tier].sort_values("walkable")
        xs = sub["walkable"].to_numpy()
        ys = sub["structural"].to_numpy()
        reg = linregress(xs, ys)
        ax.scatter(xs, ys, s=60, color="#d62728")
        xfit = np.linspace(xs.min(), xs.max(), 100)
        ax.plot(xfit, reg.slope * xfit + reg.intercept, "k--", alpha=0.6)
        ax.set_xlabel("Walkable cells")
        ax.set_ylabel("Structural cascade")
        ax.set_title(
            f"Tier {tier}: slope={reg.slope:.4f}, R²={reg.rvalue**2:.3f}",
            fontsize=10,
        )
        for _, row in sub.iterrows():
            ax.annotate(
                f"w{int(row['aisle'])}/n={int(row['num_agents'])}",
                (row["walkable"], row["structural"]),
                xytext=(5, 5), textcoords="offset points", fontsize=8,
            )
        regressions[tier] = dict(
            slope=float(reg.slope), intercept=float(reg.intercept),
            r2=float(reg.rvalue**2), p=float(reg.pvalue),
        )
    fig.suptitle("Structural cascade vs walkable area (matched density)", fontsize=11)
    fig.tight_layout()
    fig.savefig(ANA / "structural_cascade_vs_walkable.png", dpi=150)
    plt.close(fig)

    # ── Stats JSON ────────────────────────────────────────────────────
    stats = {
        "per_tier_regression": regressions,
        "raw_by_tier": {
            t: agg[agg["tier"] == t][["aisle", "num_agents", "walkable", "structural"]]
            .to_dict(orient="records")
            for t in ["L", "M", "H"]
        },
    }
    with open(ANA / "structural_cascade_stats.json", "w") as f:
        json.dump(stats, f, indent=2)

    print("Structural cascade scaling stats:")
    for t, r in regressions.items():
        print(f"  tier {t}: slope={r['slope']:.4f}, R²={r['r2']:.3f}, p={r['p']:.3e}")
    print(f"\nWrote: {ANA}/structural_cascade_heatmap.png")
    print(f"Wrote: {ANA}/structural_cascade_vs_walkable.png")
    print(f"Wrote: {ANA}/structural_cascade_stats.json")


if __name__ == "__main__":
    main()
