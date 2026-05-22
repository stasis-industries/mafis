"""
CT Rolling-Mean Impact Analysis
================================
Compares old per-tick CT (all_runs.csv from Apr 9) vs new rolling-mean CT
(all_runs.csv after running paams_full with new code).

Run AFTER: cargo test --release --test paper_experiments paams_full -- --ignored --nocapture

Usage: python3 results/analyze_ct_rolling.py
"""

import os
import sys
import numpy as np
import pandas as pd

RESULTS = os.path.dirname(os.path.abspath(__file__))

SOLVER_LABELS = {
    "pibt":          "PIBT",
    "rhcr_pbs":      "RHCR-PBS",
    "token_passing": "Token-P",
}

SCENARIO_LABELS = {
    "burst_20pct":           "Burst-20%",
    "burst_50pct":           "Burst-50%",
    "wear_medium":           "Wear-Med",
    "wear_high":             "Wear-High",
    "zone_50t":              "Zone-Out",
    "intermittent_80s80m15r":"Interm.",
}

TOPO_LABELS = {
    "warehouse_large": "WL",
    "kiva_warehouse":  "KIVA",
    "compact_grid":    "CG",
}

PAAMS_SOLVERS    = ["pibt", "rhcr_pbs", "token_passing"]
PAAMS_SCENARIOS  = list(SCENARIO_LABELS.keys())
PAAMS_TOPOLOGIES = list(TOPO_LABELS.keys())

# Default density per topology (middle agent count)
DEFAULT_N = {
    "warehouse_large": 40,
    "kiva_warehouse":  80,
    "compact_grid":    25,
}

# Excluded (solver, topology, n) — overloaded baselines
EXCLUDED = {
    ("rhcr_pbs",      "kiva_warehouse", 80),
    ("rhcr_pbs",      "kiva_warehouse", 120),
    ("token_passing", "kiva_warehouse", 120),
    ("rhcr_pbs",      "compact_grid",   40),
    ("token_passing", "compact_grid",   40),
}


def is_valid(row) -> bool:
    return (row["solver"], row["topology"], row["num_agents"]) not in EXCLUDED


def load_csv(name: str) -> pd.DataFrame | None:
    path = os.path.join(RESULTS, name)
    if not os.path.exists(path):
        return None
    return pd.read_csv(path)


def mean_ci(series: pd.Series) -> str:
    m = series.mean()
    se = series.sem()
    # 95% CI t≈1.96 for large N
    lo, hi = m - 1.96 * se, m + 1.96 * se
    return f"{m:.3f} [{lo:.3f}, {hi:.3f}]"


def ct_table(df: pd.DataFrame, label: str):
    """Print CT table: solvers × scenarios, averaged over topologies at default density.

    Applies baseline-validity filtering (matches paper methodology).
    """
    print(f"\n{'='*70}")
    print(f"  Critical Time — {label}")
    print(f"{'='*70}")

    # Faulted runs only, closest scheduler, PAAMS solvers+scenarios
    df = df[
        (df["is_baseline"] == False) &
        (df["scheduler"] == "closest") &
        df["solver"].isin(PAAMS_SOLVERS) &
        df["scenario"].isin(PAAMS_SCENARIOS)
    ].copy()

    # Default density per topology + validity filter
    rows = []
    for topo, n in DEFAULT_N.items():
        sub = df[(df["topology"] == topo) & (df["num_agents"] == n)]
        for _, row in sub.iterrows():
            if is_valid(row):
                rows.append(row)
    df_valid = pd.DataFrame(rows)

    # Average CT: first mean over seeds per (solver, topology, scenario), then mean over topologies
    topo_means = (
        df_valid.groupby(["solver", "topology", "scenario"])["critical_time"]
        .mean()
        .reset_index()
    )
    pivot = (
        topo_means.groupby(["solver", "scenario"])["critical_time"]
        .mean()
        .unstack("scenario")
    )

    # Reorder
    pivot = pivot.reindex(index=PAAMS_SOLVERS, columns=PAAMS_SCENARIOS, fill_value=float("nan"))
    pivot.index = [SOLVER_LABELS.get(s, s) for s in pivot.index]
    pivot.columns = [SCENARIO_LABELS.get(c, c) for c in pivot.columns]

    print(pivot.to_string(float_format=lambda x: f"{x:.3f}"))

    # Row averages
    print("\nRow averages (across scenarios):")
    for solver in pivot.index:
        row = pivot.loc[solver].dropna()
        print(f"  {solver:<12}: mean={row.mean():.3f}  (range {row.min():.3f}–{row.max():.3f})")

    return pivot


def solver_ordering(pivot: pd.DataFrame, label: str):
    """For each scenario, print which solver has lowest CT (best) and highest (worst)."""
    print(f"\nSolver CT ordering — {label}:")
    for col in pivot.columns:
        vals = pivot[col].sort_values()
        best  = vals.index[0]
        worst = vals.index[-1]
        print(f"  {col:<12}: best={best:<12} worst={worst:<12} | " +
              " > ".join(f"{s}={v:.3f}" for s, v in vals.items()))


def ft_table(df: pd.DataFrame, label: str):
    """Print FT table — applies same validity filter as paper.

    Sanity check: FT values should match the paper's Table 3 (rolling-mean CT
    doesn't change FT, so these values should be stable vs Apr 9 data).
    """
    print(f"\n{'='*70}")
    print(f"  Fault Tolerance (FT) — {label}  [sanity: should match paper Table 3]")
    print(f"{'='*70}")

    df = df[
        (df["is_baseline"] == False) &
        (df["scheduler"] == "closest") &
        df["solver"].isin(PAAMS_SOLVERS) &
        df["scenario"].isin(PAAMS_SCENARIOS)
    ].copy()

    rows = []
    for topo, n in DEFAULT_N.items():
        sub = df[(df["topology"] == topo) & (df["num_agents"] == n)]
        for _, row in sub.iterrows():
            if is_valid(row):
                rows.append(row)
    df_valid = pd.DataFrame(rows)

    topo_means = (
        df_valid.groupby(["solver", "topology", "scenario"])["fault_tolerance"]
        .mean()
        .reset_index()
    )
    pivot = (
        topo_means.groupby(["solver", "scenario"])["fault_tolerance"]
        .mean()
        .unstack("scenario")
    )
    pivot = pivot.reindex(index=PAAMS_SOLVERS, columns=PAAMS_SCENARIOS, fill_value=float("nan"))
    pivot.index = [SOLVER_LABELS.get(s, s) for s in pivot.index]
    pivot.columns = [SCENARIO_LABELS.get(c, c) for c in pivot.columns]

    print(pivot.to_string(float_format=lambda x: f"{x:.3f}"))
    print()
    print("  Expected (paper Table 3):")
    print("    PIBT     : 0.511  0.214  0.594  0.173  0.946  0.632")
    print("    RHCR-PBS : 1.996  0.377  0.680  0.432  0.960  3.250")
    print("    Token-P  : 0.635  0.324  0.980  0.434  1.161  0.996")


def compare_old_vs_new(new_df: pd.DataFrame):
    """
    Compare CT values from new rolling-mean run vs old per-tick Apr 9 data.
    The old data is the Apr 9 all_runs.csv which had per-tick CT.
    """
    old_path = os.path.join(RESULTS, "all_runs.csv")
    if not os.path.exists(old_path):
        print("\n[skip] all_runs.csv not found — no old/new comparison")
        return

    old_mtime = os.path.getmtime(old_path)
    print(f"\nall_runs.csv mtime: {pd.Timestamp(old_mtime, unit='s')}")

    old = pd.read_csv(old_path)
    old = old[
        (old["is_baseline"] == False) &
        (old["scheduler"] == "closest") &
        old["solver"].isin(PAAMS_SOLVERS) &
        old["scenario"].isin(PAAMS_SCENARIOS)
    ].copy()

    new = new_df[
        (new_df["is_baseline"] == False) &
        (new_df["scheduler"] == "closest") &
        new_df["solver"].isin(PAAMS_SOLVERS) &
        new_df["scenario"].isin(PAAMS_SCENARIOS)
    ].copy()

    print(f"\nOld faulted rows (3-solver filter): {len(old)}")
    print(f"New faulted rows (3-solver filter): {len(new)}")

    key_cols = ["solver", "scenario", "topology", "num_agents"]

    old_agg = old.groupby(key_cols)["critical_time"].mean().rename("ct_old")
    new_agg = new.groupby(key_cols)["critical_time"].mean().rename("ct_new")

    merged = pd.concat([old_agg, new_agg], axis=1).dropna()
    merged["delta"] = merged["ct_new"] - merged["ct_old"]
    merged["pct_change"] = (merged["delta"] / merged["ct_old"].replace(0, float("nan"))) * 100

    print(f"\nOld vs New CT (rows with data in both):")
    print(f"  Count: {len(merged)}")
    print(f"  Mean old CT:   {merged['ct_old'].mean():.4f}")
    print(f"  Mean new CT:   {merged['ct_new'].mean():.4f}")
    print(f"  Mean delta:    {merged['delta'].mean():+.4f}")
    print(f"  Mean pct chg:  {merged['pct_change'].mean():+.1f}%")
    print(f"  N increased:   {(merged['delta'] > 0).sum()} / {len(merged)}")
    print(f"  N decreased:   {(merged['delta'] < 0).sum()} / {len(merged)}")

    # Per-solver breakdown
    print("\nPer-solver delta (new − old):")
    for solver in PAAMS_SOLVERS:
        sub = merged[merged.index.get_level_values("solver") == solver]
        if len(sub) == 0:
            continue
        lbl = SOLVER_LABELS[solver]
        print(f"  {lbl:<12}: Δmean={sub['delta'].mean():+.4f}  range [{sub['delta'].min():+.4f}, {sub['delta'].max():+.4f}]")


def main():
    # ── Load new PAAMS data ───────────────────────────────────────────────────
    # paams_full writes per-topology CSVs + all_runs.csv
    new_all_path = os.path.join(RESULTS, "all_runs.csv")
    if not os.path.exists(new_all_path):
        print("ERROR: all_runs.csv not found. Run paams_full first.")
        sys.exit(1)

    print(f"Loading {new_all_path} ...")
    df = pd.read_csv(new_all_path)

    print(f"  Shape: {df.shape}")
    print(f"  Solvers: {sorted(df['solver'].unique())}")
    print(f"  Scenarios: {sorted(df['scenario'].unique())}")
    if "topology" in df.columns:
        print(f"  Topologies: {sorted(df['topology'].unique())}")
    if "num_agents" in df.columns:
        print(f"  Agent counts: {sorted(df['num_agents'].unique())}")
    print(f"  Columns: {list(df.columns)}")

    # ── CT table (new rolling-mean values) ───────────────────────────────────
    new_pivot = ct_table(df, "NEW rolling-mean W=10")
    solver_ordering(new_pivot, "NEW rolling-mean W=10")

    # ── FT sanity check ───────────────────────────────────────────────────────
    ft_table(df, "NEW")

    # ── Compare old vs new ────────────────────────────────────────────────────
    compare_old_vs_new(df)

    # ── Narrative claims check (default density only) ─────────────────────────
    print(f"\n{'='*70}")
    print("  Narrative Claims Check — DEFAULT DENSITY (matches paper narrative)")
    print(f"{'='*70}")
    print("  Paper claims: PIBT 25-46%, RHCR-PBS 16-26%, Token-P 14-20%")
    print()
    print("  Computed from CT table (range across 6 scenarios):")
    for solver_label in new_pivot.index:
        row = new_pivot.loc[solver_label].dropna()
        lo, hi = row.min() * 100, row.max() * 100
        print(f"  {solver_label:<12}: {lo:.0f}–{hi:.0f}%  (raw: {row.min():.3f}–{row.max():.3f})")

    print()
    print("  ▶ UPDATE main.tex Table 4 and CT narrative if these differ from paper claims.")
    print()

    print("\nDone.")


if __name__ == "__main__":
    main()
