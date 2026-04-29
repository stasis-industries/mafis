#!/usr/bin/env python3
"""RHCR Braess observatory proof — correlation of `pbs_partial_rate` with FT.

Reads the three-matrix output of `run_rhcr_braess_observatory_proof` from
`results/aisle_width/rhcr_braess_observatory_proof/`, computes Pearson r
between `pbs_partial_rate` and `fault_tolerance` per cell and pooled, and
emits a scatter figure plus a stats JSON for Appendix B of the paper.

Output:
- results/aisle_width/rhcr_braess_observatory_proof/proof_stats.json
- results/aisle_width/rhcr_braess_observatory_proof/rhcr_braess_observatory_proof.png
"""

import json
from pathlib import Path

import matplotlib.pyplot as plt
import numpy as np
import pandas as pd
from scipy import stats

SRC = Path("results/aisle_width/rhcr_braess_observatory_proof")

# Cell labels (topology + density) we expect
CELLS = [
    ("warehouse_single_dock", 60, "SD-w1 n=60"),
    ("warehouse_single_dock_w2", 108, "SD-w2 n=108"),
    ("warehouse_single_dock_w3", 151, "SD-w3 n=151"),
]

# Override labels produced by ExperimentConfig::rhcr_override_label()
OVERRIDE_COLORS = {
    "default": "#1f77b4",
    "h5n1": "#ff7f0e",
    "h5n6": "#2ca02c",
    "h20n1": "#d62728",
    "h20n6": "#9467bd",
}


def override_label(row: pd.Series) -> str:
    """Reconstruct the override label from CSV columns.

    The CSV doesn't carry the override label directly; we reconstruct it by
    matching the RHCR_auto() fingerprint against known corners. Simpler path
    would be to have the runner emit an `rhcr_override_label` column; for
    now we rely on the fact that all runs in a given matrix are rhcr_pbs at
    one density, so we can key off `(num_agents, seed, scenario)` and
    enumerate per-config. Each (cell, scenario, seed) batch has exactly 5
    override points in launcher order: default, h5n1, h5n6, h20n1, h20n6.
    """
    # This stub is overridden below: we read them from groupby index order.
    raise NotImplementedError


def load() -> pd.DataFrame:
    runs_files = sorted(SRC.glob("proof_*_runs.csv"))
    if not runs_files:
        raise SystemExit(
            f"no runs CSVs at {SRC}. Run the launcher first:\n"
            "  cargo test --release --lib run_rhcr_braess_observatory_proof "
            "-- --ignored --nocapture"
        )
    frames = [pd.read_csv(f) for f in runs_files]
    df = pd.concat(frames, ignore_index=True)
    # Only faulted rows have a meaningful fault_tolerance + partial_rate delta
    df = df[df["is_baseline"] == False].reset_index(drop=True)
    return df


def assign_override_labels(df: pd.DataFrame) -> pd.DataFrame:
    """Assign override label per row.

    Each (topology, num_agents, scenario, seed) block has exactly 5 rows in
    the launcher's override order: default, h5n1, h5n6, h20n1, h20n6.
    """
    order = ["default", "h5n1", "h5n6", "h20n1", "h20n6"]
    df = df.copy()
    labels: list[str] = []
    for _, group in df.groupby(
        ["topology", "num_agents", "scenario", "seed"], sort=False
    ):
        if len(group) != len(order):
            # Partial data (still running?) — label generically
            labels.extend([f"idx{i}" for i in range(len(group))])
        else:
            labels.extend(order)
    df["override_label"] = labels
    return df


def compute_correlations(df: pd.DataFrame) -> dict:
    out: dict = {"per_cell": [], "pooled": {}}
    # Pooled across all cells
    pooled_pr = df["pbs_partial_rate"].to_numpy()
    pooled_ft = df["fault_tolerance"].to_numpy()
    mask = np.isfinite(pooled_pr) & np.isfinite(pooled_ft)
    if mask.sum() >= 3:
        r, p = stats.pearsonr(pooled_pr[mask], pooled_ft[mask])
        out["pooled"] = {"r": float(r), "p": float(p), "n": int(mask.sum())}

    for topo, n_agents, label in CELLS:
        cell = df[(df["topology"] == topo) & (df["num_agents"] == n_agents)]
        if cell.empty:
            continue
        pr = cell["pbs_partial_rate"].to_numpy()
        ft = cell["fault_tolerance"].to_numpy()
        mask = np.isfinite(pr) & np.isfinite(ft)
        entry = {
            "cell": label,
            "topology": topo,
            "num_agents": int(n_agents),
            "n": int(mask.sum()),
        }
        if mask.sum() >= 3:
            r, p = stats.pearsonr(pr[mask], ft[mask])
            entry["r"] = float(r)
            entry["p"] = float(p)
        # Per-override mean FT + partial_rate
        entry["by_override"] = {}
        for ov_label, ov_group in cell.groupby("override_label"):
            entry["by_override"][str(ov_label)] = {
                "ft_mean": float(ov_group["fault_tolerance"].mean()),
                "ft_std": float(ov_group["fault_tolerance"].std()),
                "partial_rate_mean": float(ov_group["pbs_partial_rate"].mean()),
                "partial_rate_std": float(ov_group["pbs_partial_rate"].std()),
                "n": int(len(ov_group)),
            }
        out["per_cell"].append(entry)

    return out


def plot_scatter(df: pd.DataFrame, outfile: Path) -> None:
    fig, axes = plt.subplots(1, 3, figsize=(12, 3.8), sharey=True)
    for ax, (topo, n_agents, label) in zip(axes, CELLS):
        cell = df[(df["topology"] == topo) & (df["num_agents"] == n_agents)]
        for ov_label, ov_group in cell.groupby("override_label"):
            color = OVERRIDE_COLORS.get(str(ov_label), "#333333")
            ax.scatter(
                ov_group["pbs_partial_rate"],
                ov_group["fault_tolerance"],
                s=22,
                alpha=0.75,
                color=color,
                label=str(ov_label),
            )
        ax.axhline(1.0, color="black", linestyle="--", linewidth=0.8, alpha=0.6)
        ax.set_xlabel("rhcr_partial_rate")
        ax.set_title(label)
        ax.grid(True, alpha=0.3)
    axes[0].set_ylabel("Fault Tolerance (FT)")
    handles, labels_ = axes[-1].get_legend_handles_labels()
    fig.legend(
        handles,
        labels_,
        loc="center right",
        bbox_to_anchor=(1.0, 0.5),
        title="override",
        fontsize=8,
    )
    fig.suptitle(
        "RHCR-PBS: PBS partial-window rate vs Fault Tolerance (flagged Braess cells)"
    )
    fig.tight_layout(rect=[0, 0, 0.92, 0.95])
    fig.savefig(outfile, dpi=150, bbox_inches="tight")
    plt.close(fig)


def main() -> None:
    df = load()
    df = assign_override_labels(df)
    stats_out = compute_correlations(df)

    out_json = SRC / "proof_stats.json"
    out_png = SRC / "rhcr_braess_observatory_proof.png"

    with out_json.open("w") as f:
        json.dump(stats_out, f, indent=2)
    plot_scatter(df, out_png)

    print(f"wrote {out_json}")
    print(f"wrote {out_png}")
    print()
    print("=== Per-cell Pearson r (partial_rate, FT) ===")
    for entry in stats_out["per_cell"]:
        if "r" in entry:
            print(
                f"  {entry['cell']:<16} r={entry['r']:+.3f}  p={entry['p']:.3g}  "
                f"n={entry['n']}"
            )
    if "r" in stats_out["pooled"]:
        p = stats_out["pooled"]
        print(f"  {'POOLED':<16} r={p['r']:+.3f}  p={p['p']:.3g}  n={p['n']}")


if __name__ == "__main__":
    main()
