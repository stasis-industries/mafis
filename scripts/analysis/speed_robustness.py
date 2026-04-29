#!/usr/bin/env python3
"""Phase 1.D — Speed-robustness scatter.

Test whether solver-step time (computational cost) correlates with
fault-tolerance metric. 'Fast solvers aren't necessarily more robust'
would be a publishable finding if slope is negative.

Uses experiment summary CSVs columns `solver_step_us_mean` and `ft_mean`.
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from scipy.stats import linregress

RESULTS = Path("results")
OUT_DIR = RESULTS / "phase1_metaanalysis" / "speed_robustness"
OUT_DIR.mkdir(parents=True, exist_ok=True)

SOLVER_COLORS = {"pibt": "#1f77b4", "rhcr_pbs": "#ff7f0e", "token_passing": "#2ca02c"}
PERMANENT = {"burst_20pct", "burst_50pct", "wear_medium", "wear_high"}


def load():
    frames = []
    for f in [
        "warehouse_single_dock_experiment_summary.csv",
        "warehouse_dual_dock_experiment_summary.csv",
    ]:
        p = RESULTS / f
        if p.exists():
            frames.append(pd.read_csv(p))
    return pd.concat(frames, ignore_index=True)


def main():
    df = load()
    # Column may be 'solver_step_us_mean' or 'solver_step_avg_us' - check both
    step_col = "solver_step_us_mean" if "solver_step_us_mean" in df.columns else "solver_step_avg_us"
    if step_col not in df.columns:
        print(f"ERROR: no solver step-time column in summary CSV. Available: {list(df.columns)}")
        return

    df = df.dropna(subset=[step_col, "ft_mean"])
    df["fault_category"] = df["scenario"].apply(
        lambda s: "Permanent" if s in PERMANENT else "Recoverable"
    )
    print(f"{len(df)} cells with valid step-time + FT")

    # Global regression (log-scale x because step times span orders of magnitude)
    x = np.log10(df[step_col].clip(lower=1))
    y = df["ft_mean"]
    global_reg = linregress(x, y)
    print(f"\nGlobal regression (log10 step-time vs FT):")
    print(f"  slope: {global_reg.slope:.3f}")
    print(f"  intercept: {global_reg.intercept:.3f}")
    print(f"  R²: {global_reg.rvalue ** 2:.3f}")
    print(f"  p-value: {global_reg.pvalue:.3e}")

    # Per-solver regressions
    print("\nPer-solver regressions:")
    per_solver: dict = {}
    for solver in sorted(df["solver"].unique()):
        sub = df[df["solver"] == solver]
        if len(sub) < 4:
            continue
        xs = np.log10(sub[step_col].clip(lower=1))
        ys = sub["ft_mean"]
        reg = linregress(xs, ys)
        per_solver[solver] = dict(
            slope=float(reg.slope), intercept=float(reg.intercept),
            r2=float(reg.rvalue ** 2), p=float(reg.pvalue)
        )
        print(f"  {solver}: slope={reg.slope:+.3f}, R²={reg.rvalue ** 2:.3f}, p={reg.pvalue:.3e}, n={len(sub)}")

    # Scatter plot
    fig, axes = plt.subplots(1, 2, figsize=(10, 4.5), sharey=True)
    for ax, cat in zip(axes, ["Permanent", "Recoverable"]):
        sub = df[df["fault_category"] == cat]
        for solver, colour in SOLVER_COLORS.items():
            ssub = sub[sub["solver"] == solver]
            ax.scatter(
                ssub[step_col], ssub["ft_mean"],
                c=colour, alpha=0.6, label=solver, s=30,
            )
        ax.set_xscale("log")
        ax.set_xlabel("Solver step time (μs, log scale)", fontsize=9)
        ax.set_ylabel("Fault Tolerance (FT)" if cat == "Permanent" else "")
        ax.set_title(f"{cat} faults", fontsize=10)
        ax.axhline(1.0, color="grey", linestyle=":", linewidth=0.8)
        ax.legend(fontsize=8)
        ax.grid(True, alpha=0.3)

    fig.suptitle("Speed vs robustness — solver step time vs FT", fontsize=11)
    fig.tight_layout()
    fig.savefig(OUT_DIR / "scatter.png", dpi=150)
    plt.close(fig)

    # Save regression results
    with open(OUT_DIR / "regressions.json", "w") as f:
        json.dump(
            {"global": dict(slope=float(global_reg.slope), intercept=float(global_reg.intercept),
                            r2=float(global_reg.rvalue ** 2), p=float(global_reg.pvalue), n=len(df)),
             "per_solver": per_solver},
            f, indent=2,
        )
    print(f"\nWrote {OUT_DIR}/scatter.png and regressions.json")


if __name__ == "__main__":
    main()
