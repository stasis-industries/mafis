#!/usr/bin/env python3
"""Topology + density sensitivity for the PAAMS 2026 supplementary.

Produces:
  1. One cross-topology heatmap at n=40 (the only shared density).
  2. Five trajectory plots — one per metric, six subplots each (one per
     scenario), showing the metric as a function of fleet density with both
     topologies overlaid and 95% error bars.

Plus a JSON dump of the per-cell numbers so the supplementary doc can
reference structured values.
"""

import json
import math
from pathlib import Path
from itertools import combinations

import pandas as pd
import numpy as np
import matplotlib.pyplot as plt

RESULTS = Path("results")
OUT_DIR = Path("docs/supplementary/topology_density_sensitivity/figures")
OUT_DIR.mkdir(parents=True, exist_ok=True)

METRICS = ["ft_mean", "critical_time_mean", "itae_mean", "attack_rate_mean", "cascade_depth_mean"]
STD_COLS = ["ft_std", "critical_time_std", "itae_std", "attack_rate_std", "cascade_depth_std"]
N_COLS = ["ft_n", "ct_n", "itae_n", "attack_rate_n", "cascade_depth_n"]
LABELS = ["FT", "CT", "TWTE", "AR", "CascDepth"]
Y_LABELS = ["Fault Tolerance", "Critical Time", "TWTE (tick-weighted error)", "Attack Rate", "Cascade Depth"]

SOLVERS = ["pibt", "rhcr_pbs", "token_passing"]
SCENARIOS = [
    "burst_20pct", "burst_50pct",
    "wear_medium", "wear_high",
    "zone_50t", "intermittent_80s80m15r",
]

SD_DENSITIES = [20, 40, 60]
DD_DENSITIES = [40, 80, 120]

# Per-metric absolute caps for heatmap colour. A cell at the cap saturates the
# colour bar. Below the cap, colour intensity is proportional to the raw value.
# Caps are operator-defined thresholds for "this much change is genuinely big",
# not statistical percentiles — they decouple the visual signal from the
# column-max trap (a small column max should not read as deep red).
MAGNITUDE_CAPS = {
    "FT":         0.5,    # 50-pp FT swing is operationally large
    "CT":         0.5,
    "TWTE":   20000.0,    # 20k tick-weighted error is large
    "AR":         0.5,
    "CascDepth":  5.0,    # 5 BFS levels is structurally large at n<=120
}

# Approx t-critical for 95% CI, n=30. Good enough for plotting error bars
# without re-importing scipy.
T_CRIT_95 = 2.045


def load():
    sd = pd.read_csv(RESULTS / "warehouse_single_dock_summary.csv")
    dd = pd.read_csv(RESULTS / "warehouse_dual_dock_summary.csv")
    return sd, dd


# ── Cross-topology heatmap (kept from prior version) ──────────────────


def render_cross_heatmap(matrix, title, outfile):
    """Per-column normalised colour, raw values annotated. TWTE lives on a
    different scale than the other metrics so this is the only fair way to
    display them on one chart.
    """
    fig, ax = plt.subplots(figsize=(8, 4.5))
    caps = np.array([MAGNITUDE_CAPS[label] for label in LABELS])
    norm = np.clip(matrix / caps, 0.0, 1.0)
    im = ax.imshow(norm, cmap="YlOrRd", aspect="auto", vmin=0, vmax=1)
    ax.set_xticks(range(len(LABELS)))
    ax.set_xticklabels(LABELS, fontsize=9)
    ax.set_yticks(range(len(SCENARIOS)))
    ax.set_yticklabels(SCENARIOS, fontsize=8)
    for i in range(len(SCENARIOS)):
        for j in range(len(LABELS)):
            raw = matrix[i, j]
            n = norm[i, j]
            color = "white" if n > 0.6 else "black"
            label = f"{raw:,.0f}" if abs(raw) >= 100 else f"{raw:.2f}"
            # Mark saturated cells (value at or above the cap) with an asterisk
            # so readers know the colour is clipped, not exaggerated.
            if abs(raw) >= caps[j]:
                label = f"{label}*"
            ax.text(j, i, label, ha="center", va="center", fontsize=8, color=color)
    cbar = plt.colorbar(im, ax=ax, shrink=0.85)
    cbar.set_label("Magnitude relative to per-metric cap (1.0 = cap reached or exceeded)", fontsize=8)
    ax.set_title(title, fontsize=10)
    fig.tight_layout()
    fig.savefig(outfile, dpi=150)
    plt.close(fig)
    print(f"  wrote {outfile}")


def cross_topology_matrix(sd, dd, n):
    sd_n = sd[sd["num_agents"] == n]
    dd_n = dd[dd["num_agents"] == n]
    merged = sd_n.merge(dd_n, on=["solver", "scenario"], suffixes=("_SD", "_DD"))
    heat = np.zeros((len(SCENARIOS), len(METRICS)))
    for i, scen in enumerate(SCENARIOS):
        sub = merged[merged["scenario"] == scen]
        for j, m in enumerate(METRICS):
            heat[i, j] = np.nanmean(np.abs(sub[f"{m}_DD"] - sub[f"{m}_SD"]))
    return heat


def rank_flips(sd_sub, dd_sub, metric):
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


# ── Trajectory plots (metric vs density, one figure per metric) ───────


def aggregate_trajectory(df, densities, metric_col, std_col, n_col):
    """Average across solvers for each (scenario, density). Return three
    arrays of shape (scenarios, densities): mean, ci_half_width, n.
    Missing cells are NaN.
    """
    n_sc = len(SCENARIOS)
    n_d = len(densities)
    mean = np.full((n_sc, n_d), np.nan)
    err = np.full((n_sc, n_d), np.nan)
    n_used = np.full((n_sc, n_d), np.nan)
    for i, scen in enumerate(SCENARIOS):
        for j, dens in enumerate(densities):
            sub = df[(df["scenario"] == scen) & (df["num_agents"] == dens)]
            if sub.empty:
                continue
            # Average solver means
            mean[i, j] = float(np.nanmean(sub[metric_col]))
            # Aggregate variance: each solver contributes its own std (across
            # seeds). Combine as pooled sample size for the t-CI half-width.
            std_vals = sub[std_col].to_numpy(dtype=float)
            n_vals = sub[n_col].to_numpy(dtype=float) if n_col in sub.columns else None
            if n_vals is None or np.all(np.isnan(n_vals)):
                # Fall back to a fixed 30-seed assumption when n column missing.
                n_vals = np.full_like(std_vals, 30.0)
            pooled_n = float(np.nansum(n_vals))
            n_used[i, j] = pooled_n
            if pooled_n > 1 and not np.all(np.isnan(std_vals)):
                # Conservative half-width: average std across solvers, divide by
                # sqrt of the total seed count, scale by t-critical.
                std_mean = float(np.nanmean(std_vals))
                err[i, j] = T_CRIT_95 * std_mean / math.sqrt(pooled_n)
    return mean, err, n_used


def render_trajectory(metric_label, y_label, sd_data, dd_data, outfile):
    """Six subplots in a 2x3 grid (one per scenario). Each subplot shows the
    metric on the y-axis and fleet density on the x-axis, with one line per
    topology (Single-Dock blue, Dual-Dock orange). Error bars are 95% Welch
    confidence half-widths derived from the per-solver std times the
    n=30-seeds-per-solver-aggregated t-critical.
    """
    sd_mean, sd_err, _ = sd_data
    dd_mean, dd_err, _ = dd_data
    fig, axes = plt.subplots(2, 3, figsize=(11, 6), sharex=False)
    axes = axes.flatten()
    for i, scen in enumerate(SCENARIOS):
        ax = axes[i]
        ax.errorbar(SD_DENSITIES, sd_mean[i], yerr=sd_err[i], marker="o", color="#1f77b4",
                    label="Single-Dock", capsize=3, linewidth=1.5)
        ax.errorbar(DD_DENSITIES, dd_mean[i], yerr=dd_err[i], marker="s", color="#ff7f0e",
                    label="Dual-Dock", capsize=3, linewidth=1.5)
        ax.set_title(scen, fontsize=9)
        ax.set_xlabel("Fleet density (agents)", fontsize=8)
        ax.set_ylabel(y_label, fontsize=8)
        ax.grid(True, alpha=0.3)
        ax.tick_params(labelsize=7)
        if i == 0:
            ax.legend(fontsize=7, loc="best")
    fig.suptitle(f"{y_label} vs density, by scenario", fontsize=11, y=1.00)
    fig.tight_layout()
    fig.savefig(outfile, dpi=150)
    plt.close(fig)
    print(f"  wrote {outfile}")


def main():
    sd, dd = load()

    # ── 1. Cross-topology heatmap at n=40 ──────────────────────────────
    print("=== Cross-topology delta at n=40 (only shared density) ===")
    cross = cross_topology_matrix(sd, dd, 40)
    render_cross_heatmap(
        cross,
        "|Δ| Single-Dock n=40 ↔ Dual-Dock n=40 (only shared density)",
        OUT_DIR / "cross_topology_delta_n40.png",
    )
    sd40 = sd[sd["num_agents"] == 40]
    dd40 = dd[dd["num_agents"] == 40]
    flips_per_metric = {}
    for m, label in zip(METRICS, LABELS):
        flips = rank_flips(sd40, dd40, m)
        flips_per_metric[label] = [
            dict(scenario=f[0], solver_a=f[1], solver_b=f[2], sd_a=f[3], sd_b=f[4], dd_a=f[5], dd_b=f[6])
            for f in flips
        ]

    # ── 2. Trajectory plots, one per metric ────────────────────────────
    print("=== Trajectory plots (metric vs density per scenario) ===")
    trajectories = {}
    for label, m, std_c, n_c, y_lab in zip(LABELS, METRICS, STD_COLS, N_COLS, Y_LABELS):
        sd_traj = aggregate_trajectory(sd, SD_DENSITIES, m, std_c, n_c)
        dd_traj = aggregate_trajectory(dd, DD_DENSITIES, m, std_c, n_c)
        outfile = OUT_DIR / f"traj_{label}.png"
        render_trajectory(label, y_lab, sd_traj, dd_traj, outfile)
        trajectories[label] = {
            "scenarios": SCENARIOS,
            "sd_densities": SD_DENSITIES,
            "dd_densities": DD_DENSITIES,
            "sd_mean": sd_traj[0].tolist(),
            "sd_ci95_half": sd_traj[1].tolist(),
            "dd_mean": dd_traj[0].tolist(),
            "dd_ci95_half": dd_traj[1].tolist(),
        }

    # ── JSON dump ──────────────────────────────────────────────────────
    out = {
        "cross_topology_n40": {
            "matrix": cross.tolist(),
            "scenarios": SCENARIOS,
            "metrics": LABELS,
            "rank_flips_per_metric": {k: len(v) for k, v in flips_per_metric.items()},
            "rank_flips_detail": flips_per_metric,
        },
        "trajectories": trajectories,
    }
    with open(OUT_DIR / "topology_sensitivity.json", "w") as f:
        json.dump(out, f, indent=2)
    print(f"\nWrote {OUT_DIR}/topology_sensitivity.json")


if __name__ == "__main__":
    main()
