#!/usr/bin/env python3
"""Phase 1.C — Topology sensitivity quantification.

Compare Single-Dock vs Dual-Dock results cell-by-cell. Detect:
- rank flips between solvers across topologies
- metric shift magnitude vs CI width (shift / noise ratio)

Decision signal:
- flip count > 0 with shift/noise > 1 per metric → topology-sensitivity is real
- flip count = 0 or shifts < noise → rankings are topology-stable at this density
"""

import json
from pathlib import Path
from itertools import combinations

import pandas as pd
import numpy as np
import matplotlib.pyplot as plt

RESULTS = Path("/Users/teddyadmin/Developments/Research-Project/mafis/results")
OUT_DIR = RESULTS / "phase1_metaanalysis" / "topology_sensitivity"
OUT_DIR.mkdir(parents=True, exist_ok=True)

METRICS = ["ft_mean", "critical_time_mean", "itae_mean", "attack_rate_mean", "cascade_depth_mean"]
LABELS = ["FT", "CT", "ITAE", "AR", "CascDepth"]
CI_LO = {"ft_mean": "ft_ci95_lo"}

SOLVERS = ["pibt", "rhcr_pbs", "token_passing"]
SCENARIOS = [
    "burst_20pct", "burst_50pct",
    "wear_medium", "wear_high",
    "zone_50t", "intermittent_80s80m15r",
]


def load():
    sd = pd.read_csv(RESULTS / "paams_warehouse_single_dock_summary.csv")
    dd = pd.read_csv(RESULTS / "paams_warehouse_dual_dock_summary.csv")
    return sd, dd


def match_cells_at_density(sd, dd, n):
    sd_n = sd[sd["num_agents"] == n].copy()
    dd_n = dd[dd["num_agents"] == n].copy()
    keys = ["solver", "scenario"]
    merged = sd_n.merge(dd_n, on=keys, suffixes=("_SD", "_DD"))
    return merged, sd_n, dd_n


def rank_flips(sd_sub, dd_sub, metric):
    """For each scenario, compare solver ranking under SD vs DD.
    A flip = any pair (a,b) where rank_SD(a) < rank_SD(b) but rank_DD(a) > rank_DD(b)."""
    flips = []
    for scen in SCENARIOS:
        sd_vals = {s: sd_sub.loc[(sd_sub["solver"] == s) & (sd_sub["scenario"] == scen), metric].iloc[0]
                   for s in SOLVERS if not sd_sub.loc[(sd_sub["solver"] == s) & (sd_sub["scenario"] == scen)].empty}
        dd_vals = {s: dd_sub.loc[(dd_sub["solver"] == s) & (dd_sub["scenario"] == scen), metric].iloc[0]
                   for s in SOLVERS if not dd_sub.loc[(dd_sub["solver"] == s) & (dd_sub["scenario"] == scen)].empty}
        common = set(sd_vals) & set(dd_vals)
        for a, b in combinations(sorted(common), 2):
            sd_cmp = np.sign(sd_vals[a] - sd_vals[b])
            dd_cmp = np.sign(dd_vals[a] - dd_vals[b])
            if sd_cmp != 0 and dd_cmp != 0 and sd_cmp != dd_cmp:
                flips.append((scen, a, b, sd_vals[a], sd_vals[b], dd_vals[a], dd_vals[b]))
    return flips


def main():
    sd, dd = load()

    # Only n=40 appears in BOTH matrices — use it for matched-density comparison.
    # This isolates topology-effect from density-effect.
    merged, sd40, dd40 = match_cells_at_density(sd, dd, 40)

    print(f"SD n=40 cells: {len(sd40)}")
    print(f"DD n=40 cells: {len(dd40)}")
    deltas = {}
    for m, label in zip(METRICS, LABELS):
        d_col = f"{m}_SD"
        e_col = f"{m}_DD"
        merged[f"delta_{label}"] = merged[e_col] - merged[d_col]
        deltas[label] = {
            "mean_abs": float(np.nanmean(np.abs(merged[f"delta_{label}"]))),
            "max_abs": float(np.nanmax(np.abs(merged[f"delta_{label}"]))),
            "median_abs": float(np.nanmedian(np.abs(merged[f"delta_{label}"]))),
        }
    print("\n=== |Δ| between SD and DD at matched density ===")
    for label, stats in deltas.items():
        print(f"  {label:12s}  mean|Δ|={stats['mean_abs']:.3f}  med|Δ|={stats['median_abs']:.3f}  max|Δ|={stats['max_abs']:.3f}")

    # ── Shift/noise ratio for FT (uses CI width as noise proxy)
    # CI width on FT = ft_ci95_hi - ft_ci95_lo
    merged["ft_ci_width_SD"] = merged["ft_ci95_hi_SD"] - merged["ft_ci95_lo_SD"]
    merged["ft_ci_width_DD"] = merged["ft_ci95_hi_DD"] - merged["ft_ci95_lo_DD"]
    merged["ft_ci_width_avg"] = (merged["ft_ci_width_SD"] + merged["ft_ci_width_DD"]) / 2.0
    merged["ft_shift_over_noise"] = np.abs(merged["delta_FT"]) / merged["ft_ci_width_avg"].replace(0, np.nan)
    print("\n=== FT shift/noise ratio (|Δ_FT| / avg CI width) ===")
    print(f"  mean: {merged['ft_shift_over_noise'].mean():.3f}")
    print(f"  median: {merged['ft_shift_over_noise'].median():.3f}")
    print(f"  cells where shift > noise: {(merged['ft_shift_over_noise'] > 1).sum()} / {len(merged)}")

    # ── Rank flips per metric (matched density n=40)
    print("\n=== Rank flips per metric (Single-Dock n=40 vs Dual-Dock n=40) ===")
    all_flips = {}
    for m, label in zip(METRICS, LABELS):
        flips = rank_flips(sd40, dd40, m)
        all_flips[label] = [
            dict(scenario=f[0], solver_a=f[1], solver_b=f[2], sd_a=f[3], sd_b=f[4], dd_a=f[5], dd_b=f[6])
            for f in flips
        ]
        print(f"  {label:12s}  flips = {len(flips)}")
        for f in flips[:3]:
            scen, a, b, sa, sb, da, db = f
            print(f"    {scen} / {a} vs {b}: SD ({sa:.3f} vs {sb:.3f}) → DD ({da:.3f} vs {db:.3f})")

    with open(OUT_DIR / "topology_sensitivity.json", "w") as f:
        json.dump({"deltas": deltas, "flips_per_metric": {k: len(v) for k, v in all_flips.items()},
                   "flips_detail": all_flips}, f, indent=2)

    # ── Heatmap of |Δ| per (scenario, metric)
    fig, ax = plt.subplots(figsize=(8, 4.5))
    heat = np.zeros((len(SCENARIOS), len(METRICS)))
    for i, scen in enumerate(SCENARIOS):
        sub = merged[merged["scenario"] == scen]
        for j, m in enumerate(METRICS):
            heat[i, j] = np.nanmean(np.abs(sub[f"delta_{LABELS[j]}"]))

    im = ax.imshow(heat, cmap="YlOrRd", aspect="auto")
    ax.set_xticks(range(len(METRICS)))
    ax.set_xticklabels(LABELS, fontsize=9)
    ax.set_yticks(range(len(SCENARIOS)))
    ax.set_yticklabels(SCENARIOS, fontsize=8)
    for i in range(len(SCENARIOS)):
        for j in range(len(METRICS)):
            v = heat[i, j]
            color = "white" if v > heat.max() * 0.6 else "black"
            ax.text(j, i, f"{v:.2f}", ha="center", va="center", fontsize=8, color=color)
    plt.colorbar(im, ax=ax, shrink=0.85)
    ax.set_title("|Δ| per (scenario, metric): Single-Dock n=40 ↔ Dual-Dock n=40", fontsize=10)
    fig.tight_layout()
    fig.savefig(OUT_DIR / "delta_heatmap.png", dpi=150)
    plt.close(fig)
    print(f"\nWrote {OUT_DIR}/delta_heatmap.png")
    print(f"Done. Outputs in {OUT_DIR}")


if __name__ == "__main__":
    main()
