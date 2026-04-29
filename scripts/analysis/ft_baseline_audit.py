#!/usr/bin/env python3
"""FT + baseline-validity audit across the aisle-width sweep.

Flags overloaded-baseline cells (FT >> 1 = baseline throughput collapsed,
fault ends up faster than no-fault — classic Braess / gridlock signal).
Computes CI-width flag per cell and aggregates cross-aisle FT stability.

Outputs:
- `results/aisle_width/analysis/ft_by_aisle.png`
- `results/aisle_width/analysis/ft_audit_flags.csv`
- `results/aisle_width/analysis/ft_audit_stats.json`
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

OUT = Path("results/aisle_width")
if (OUT / "merged_post_fix").exists():
    SRC = OUT / "merged_post_fix"
elif (OUT / "merged").exists():
    SRC = OUT / "merged"
else:
    SRC = OUT
ANA = OUT / "analysis"
ANA.mkdir(parents=True, exist_ok=True)

AISLE = {
    "warehouse_single_dock": 1,
    "warehouse_single_dock_w2": 2,
    "warehouse_single_dock_w3": 3,
}
TIER = {20: "L", 40: "M", 60: "H", 36: "L", 72: "M", 108: "H", 50: "L", 100: "M", 151: "H"}
COLORS = {"pibt": "#1f77b4", "rhcr_pbs": "#ff7f0e", "token_passing": "#2ca02c"}
LABELS = {"pibt": "PIBT", "rhcr_pbs": "RHCR-PBS", "token_passing": "Token Passing"}

# CI width threshold — anything wider than this is suspicious (high variance).
CI_WIDTH_THRESHOLD = 0.30
# FT Braess-flag threshold.
FT_BRAESS_THRESHOLD = 1.20


def load() -> pd.DataFrame:
    frames = [pd.read_csv(f) for f in sorted(SRC.glob("*_summary.csv"))]
    df = pd.concat(frames, ignore_index=True)
    df["aisle"] = df["topology"].map(AISLE)
    df["tier"] = df["num_agents"].map(TIER)
    df["ft_ci_width"] = df["ft_ci95_hi"] - df["ft_ci95_lo"]
    df["braess_flag"] = df["ft_mean"] > FT_BRAESS_THRESHOLD
    df["wide_ci_flag"] = df["ft_ci_width"] > CI_WIDTH_THRESHOLD
    return df


def main():
    df = load()

    # ── Line plot: FT vs aisle per solver/tier ────────────────────────
    fig, axes = plt.subplots(1, 3, figsize=(12, 4), sharey=True)
    for ax, tier in zip(axes, ["L", "M", "H"]):
        for solver in ["pibt", "rhcr_pbs", "token_passing"]:
            sub = (
                df[(df["solver"] == solver) & (df["tier"] == tier)]
                .groupby("aisle", as_index=False)
                .agg(ft=("ft_mean", "mean"), ci=("ft_ci_width", "mean"))
                .sort_values("aisle")
            )
            if sub.empty:
                continue
            ax.errorbar(
                sub["aisle"], sub["ft"], yerr=sub["ci"] / 2,
                fmt="o-", color=COLORS[solver], label=LABELS[solver],
                linewidth=2, markersize=8, capsize=4,
            )
        ax.axhline(1.0, color="grey", linestyle=":", linewidth=0.8)
        ax.set_xlabel("Aisle width")
        ax.set_xticks([1, 2, 3])
        ax.set_title(f"Tier {tier}", fontsize=10)
        ax.grid(True, alpha=0.3)
    axes[0].set_ylabel("FT (faulted / baseline throughput)")
    axes[0].legend(fontsize=9, loc="lower left")
    fig.suptitle(
        "FT by aisle width — aggregate metric is insensitive to topology",
        fontsize=11,
    )
    fig.tight_layout()
    fig.savefig(ANA / "ft_by_aisle.png", dpi=150)
    plt.close(fig)

    # ── Flag table: cells with overloaded baseline OR wide CI ────────
    flags = df[df["braess_flag"] | df["wide_ci_flag"]][[
        "solver", "topology", "num_agents", "scenario",
        "ft_mean", "ft_ci95_lo", "ft_ci95_hi", "ft_ci_width",
        "braess_flag", "wide_ci_flag",
    ]].sort_values(["ft_mean"], ascending=False)
    flags.to_csv(ANA / "ft_audit_flags.csv", index=False)

    # ── Stats JSON ────────────────────────────────────────────────────
    stats = {
        "total_cells": int(len(df)),
        "braess_flagged_cells": int(df["braess_flag"].sum()),
        "wide_ci_flagged_cells": int(df["wide_ci_flag"].sum()),
        "either_flagged": int((df["braess_flag"] | df["wide_ci_flag"]).sum()),
        "flagged_by_solver": df.groupby("solver")["braess_flag"].sum().to_dict(),
        "mean_ft_by_aisle_solver": {
            f"{s}_w{a}": float(
                df[(df["solver"] == s) & (df["aisle"] == a) & ~df["braess_flag"]]["ft_mean"].mean()
            )
            for s in ["pibt", "rhcr_pbs", "token_passing"]
            for a in [1, 2, 3]
        },
    }
    with open(ANA / "ft_audit_stats.json", "w") as f:
        json.dump(stats, f, indent=2)

    print(f"Total cells: {stats['total_cells']}")
    print(f"Braess-flagged (FT > {FT_BRAESS_THRESHOLD}): {stats['braess_flagged_cells']}")
    print(f"Wide-CI-flagged (CI > {CI_WIDTH_THRESHOLD}): {stats['wide_ci_flagged_cells']}")
    print(f"Either flag: {stats['either_flagged']} ({100*stats['either_flagged']/stats['total_cells']:.1f}%)")
    print("\nBraess flags by solver:")
    for s, c in stats["flagged_by_solver"].items():
        print(f"  {LABELS.get(s, s):<15} {int(c)}")
    print(f"\nWrote: {ANA}/ft_by_aisle.png")
    print(f"Wrote: {ANA}/ft_audit_flags.csv")
    print(f"Wrote: {ANA}/ft_audit_stats.json")


if __name__ == "__main__":
    main()
