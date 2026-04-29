#!/usr/bin/env python3
"""Phase 1.A — Metric correlation matrix.

Reads Single-Dock + Dual-Dock + scheduler_effect summary CSVs and computes
Pearson + Spearman correlations between the 5 non-Rapidity metrics.
Rapidity is excluded per path β (pre-fix CSV values are stale after the
2026-04-17 degradation-observed gate).

Stratifies by fault category (permanent vs recoverable) because Cascade
Depth is mechanically 0 on permanent faults and would dominate the raw
global correlation.

Decision signal:
- any |r| > 0.85 → that pair is redundant (paper's '5 independent metrics'
  claim needs adjustment)
- all |r| < 0.5  → 5 independent axes confirmed (tool-paper core claim holds)
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from scipy.stats import pearsonr, spearmanr

RESULTS = Path("results")
OUT_DIR = RESULTS / "phase1_metaanalysis" / "metric_correlation"
OUT_DIR.mkdir(parents=True, exist_ok=True)

METRICS = [
    "ft_mean",
    "critical_time_mean",
    "itae_mean",
    "attack_rate_mean",
    "cascade_depth_mean",
]
LABELS = ["FT", "CT", "ITAE", "AR", "CascDepth"]

PERMANENT = {"burst_20pct", "burst_50pct", "wear_medium", "wear_high"}
RECOVERABLE = {"zone_50t", "intermittent_80s80m15r"}


def load():
    frames = []
    for f in [
        "warehouse_single_dock_experiment_summary.csv",
        "warehouse_dual_dock_experiment_summary.csv",
        "scheduler_effect_experiment_summary.csv",
    ]:
        p = RESULTS / f
        if p.exists():
            frames.append(pd.read_csv(p))
    df = pd.concat(frames, ignore_index=True)
    return df


def corr_matrix(sub: pd.DataFrame, method: str) -> pd.DataFrame:
    n = len(METRICS)
    mat = np.full((n, n), np.nan)
    for i, a in enumerate(METRICS):
        for j, b in enumerate(METRICS):
            if i == j:
                mat[i, j] = 1.0
                continue
            x = sub[a].dropna()
            y = sub[b].dropna()
            common = x.index.intersection(y.index)
            x = x.loc[common].to_numpy()
            y = y.loc[common].to_numpy()
            if len(x) < 3 or np.std(x) == 0 or np.std(y) == 0:
                continue
            r = pearsonr(x, y)[0] if method == "pearson" else spearmanr(x, y)[0]
            mat[i, j] = r
    return pd.DataFrame(mat, index=LABELS, columns=LABELS)


def plot_heatmap(mat: pd.DataFrame, title: str, out_path: Path):
    fig, ax = plt.subplots(figsize=(5.5, 4.5))
    im = ax.imshow(mat.values, cmap="RdBu_r", vmin=-1, vmax=1, aspect="auto")
    ax.set_xticks(range(len(LABELS)))
    ax.set_xticklabels(LABELS, fontsize=9)
    ax.set_yticks(range(len(LABELS)))
    ax.set_yticklabels(LABELS, fontsize=9)
    for i in range(len(LABELS)):
        for j in range(len(LABELS)):
            v = mat.values[i, j]
            if np.isnan(v):
                ax.text(j, i, "—", ha="center", va="center", fontsize=8)
            else:
                color = "white" if abs(v) > 0.6 else "black"
                ax.text(j, i, f"{v:.2f}", ha="center", va="center", fontsize=9, color=color)
    plt.colorbar(im, ax=ax, shrink=0.85)
    ax.set_title(title, fontsize=10)
    fig.tight_layout()
    fig.savefig(out_path, dpi=150)
    plt.close(fig)


def main():
    df = load()
    print(f"loaded {len(df)} cells from summary CSVs")

    perm = df[df["scenario"].isin(PERMANENT)].copy()
    rec = df[df["scenario"].isin(RECOVERABLE)].copy()

    print(f"  permanent cells:  {len(perm)}")
    print(f"  recoverable cells: {len(rec)}")

    results: dict = {}

    for name, sub in [("permanent", perm), ("recoverable", rec), ("all", df)]:
        for method in ("pearson", "spearman"):
            mat = corr_matrix(sub, method)
            key = f"{name}_{method}"
            results[key] = mat.round(3).to_dict()
            out_path = OUT_DIR / f"corr_{name}_{method}.png"
            plot_heatmap(mat, f"{method.capitalize()} — {name} ({len(sub)} cells)", out_path)
            print(f"  wrote {out_path.name}")

    # Flag strong correlations outside the diagonal
    print("\n=== Pairwise flags (|r| > 0.85 in either permanent or recoverable) ===")
    for name in ("permanent", "recoverable"):
        for method in ("pearson", "spearman"):
            key = f"{name}_{method}"
            m = results[key]
            for i, a in enumerate(LABELS):
                for j, b in enumerate(LABELS):
                    if i >= j:
                        continue
                    v = m[a][b]
                    if v is None or np.isnan(v):
                        continue
                    if abs(v) > 0.85:
                        print(f"  [{key}] {a}↔{b}: r = {v:.3f}")

    with open(OUT_DIR / "correlations.json", "w") as f:
        json.dump({k: {a: dict(v) for a, v in mat.items()} for k, mat in results.items()}, f, indent=2, default=float)

    print(f"\nDone. Outputs in {OUT_DIR}")


if __name__ == "__main__":
    main()
