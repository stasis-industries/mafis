"""Pairwise solver comparison with Benjamini-Hochberg FDR correction.

For each (matrix, topology, scenario, scheduler, num_agents, metric), run
Welch's t-test between every pair of solvers using per-seed faulted-run
values. Collect all p-values across the family, then apply BH FDR at
q=0.05 to flag the comparisons that survive multiple-comparison correction.

Output: results/fdr_adjusted_pvalues.csv
"""
from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import pandas as pd
from scipy import stats

REPO_ROOT = Path(__file__).resolve().parents[2]
RESULTS = REPO_ROOT / "results"

# Map (label) -> per-run CSV path, restricted to canonical post-fix data
INPUT_MATRICES = {
    "E1_warehouse_single_dock": RESULTS / "warehouse_single_dock_runs.csv",
    "E1_warehouse_dual_dock":   RESULTS / "warehouse_dual_dock_runs.csv",
    "E2_scheduler":             RESULTS / "scheduler_effect_runs.csv",
    "aux_SDw1":                 RESULTS / "aisle_width" / "merged_post_fix" / "aisle_width_w1_runs.csv",
    "aux_SDw2_inenv":           RESULTS / "aisle_width" / "merged_post_fix" / "aisle_width_w2_in_env_runs.csv",
    "aux_SDw2_outenv":          RESULTS / "aisle_width" / "merged_post_fix" / "aisle_width_w2_out_env_runs.csv",
    "aux_SDw3_inenv":           RESULTS / "aisle_width" / "merged_post_fix" / "aisle_width_w3_in_env_runs.csv",
    "aux_SDw3_outenv":          RESULTS / "aisle_width" / "merged_post_fix" / "aisle_width_w3_out_env_runs.csv",
}

# Metrics tested (paper's six primary metrics)
METRICS = [
    "fault_tolerance",
    "critical_time",
    "itae",
    "attack_rate",
    "cascade_depth_avg",
    "rapidity",
]

# Solvers compared (canonical 3-way)
SOLVERS = ("pibt", "rhcr_pbs", "token_passing")
PAIRS = [(SOLVERS[i], SOLVERS[j]) for i in range(len(SOLVERS)) for j in range(i + 1, len(SOLVERS))]

GROUP_COLS = ["topology", "scenario", "scheduler", "num_agents"]

OUTPUT_PATH = RESULTS / "fdr_adjusted_pvalues.csv"
Q_LEVEL = 0.05


@dataclass
class ComparisonRow:
    matrix: str
    topology: str
    scenario: str
    scheduler: str
    num_agents: int
    metric: str
    solver_a: str
    solver_b: str
    n_a: int
    n_b: int
    mean_a: float
    mean_b: float
    mean_diff: float
    t_stat: float
    p_raw: float
    cliffs_d: float


def cliffs_delta(a: np.ndarray, b: np.ndarray) -> float:
    """Cliff's delta for two independent samples. O(n*m) but n,m ~ 30."""
    if len(a) == 0 or len(b) == 0:
        return float("nan")
    greater = sum(1 for x in a for y in b if x > y)
    less = sum(1 for x in a for y in b if x < y)
    return (greater - less) / (len(a) * len(b))


def load_matrix(label: str, path: Path) -> pd.DataFrame | None:
    if not path.exists():
        print(f"[skip] {label}: {path} not found")
        return None
    df = pd.read_csv(path)
    # Filter to faulted runs only — differential metrics already computed
    if "is_baseline" not in df.columns:
        print(f"[warn] {label}: missing is_baseline column, including all rows")
    else:
        # CSV uses lowercase true/false strings or bools
        mask = df["is_baseline"].astype(str).str.lower() == "false"
        df = df.loc[mask].copy()
    df["__matrix__"] = label
    return df


def run_pairwise_tests(df: pd.DataFrame, matrix: str) -> list[ComparisonRow]:
    rows: list[ComparisonRow] = []
    # Group by configuration cell
    group_keys = [c for c in GROUP_COLS if c in df.columns]
    if not group_keys:
        return rows
    for keys, sub in df.groupby(group_keys, dropna=False):
        kdict = dict(zip(group_keys, keys if isinstance(keys, tuple) else (keys,)))
        for metric in METRICS:
            if metric not in sub.columns:
                continue
            for sa, sb in PAIRS:
                xa = sub.loc[sub["solver"] == sa, metric].dropna()
                xb = sub.loc[sub["solver"] == sb, metric].dropna()
                # Rapidity is NaN-heavy for permanent faults — skip when sample too small
                if len(xa) < 5 or len(xb) < 5:
                    continue
                # Skip degenerate constant samples (t-test undefined)
                if xa.var(ddof=1) == 0 and xb.var(ddof=1) == 0 and xa.mean() == xb.mean():
                    continue
                try:
                    t, p = stats.ttest_ind(xa.values, xb.values, equal_var=False, nan_policy="omit")
                except Exception:
                    continue
                if not math.isfinite(p):
                    continue
                d = cliffs_delta(xa.values, xb.values)
                rows.append(
                    ComparisonRow(
                        matrix=matrix,
                        topology=str(kdict.get("topology", "")),
                        scenario=str(kdict.get("scenario", "")),
                        scheduler=str(kdict.get("scheduler", "")),
                        num_agents=int(kdict.get("num_agents", 0) or 0),
                        metric=metric,
                        solver_a=sa,
                        solver_b=sb,
                        n_a=int(len(xa)),
                        n_b=int(len(xb)),
                        mean_a=float(xa.mean()),
                        mean_b=float(xb.mean()),
                        mean_diff=float(xa.mean() - xb.mean()),
                        t_stat=float(t),
                        p_raw=float(p),
                        cliffs_d=float(d),
                    )
                )
    return rows


def bh_adjust(pvals: np.ndarray) -> np.ndarray:
    """Benjamini-Hochberg adjusted p-values (no library dependency).

    Returns array of BH-adjusted p-values in the original order.
    """
    n = len(pvals)
    if n == 0:
        return pvals
    order = np.argsort(pvals)
    ranked = pvals[order]
    adj = ranked * n / (np.arange(n) + 1)
    # Enforce monotonicity from the largest rank downward
    adj = np.minimum.accumulate(adj[::-1])[::-1]
    adj = np.clip(adj, 0.0, 1.0)
    out = np.empty_like(adj)
    out[order] = adj
    return out


def main() -> None:
    all_rows: list[ComparisonRow] = []
    for label, path in INPUT_MATRICES.items():
        df = load_matrix(label, path)
        if df is None:
            continue
        rows = run_pairwise_tests(df, label)
        print(f"[ok] {label}: {len(rows)} comparisons")
        all_rows.extend(rows)

    if not all_rows:
        raise SystemExit("No comparisons collected — check input CSVs")

    out = pd.DataFrame([r.__dict__ for r in all_rows])
    out["p_bh"] = bh_adjust(out["p_raw"].to_numpy())
    out["significant_bh_q05"] = out["p_bh"] < Q_LEVEL
    # Sort for reviewer scan
    out = out.sort_values(
        ["matrix", "topology", "num_agents", "scenario", "metric", "solver_a", "solver_b"]
    ).reset_index(drop=True)
    out.to_csv(OUTPUT_PATH, index=False)

    n = len(out)
    n_sig_raw = int((out["p_raw"] < Q_LEVEL).sum())
    n_sig_bh = int(out["significant_bh_q05"].sum())
    print(f"\nWrote {n} comparisons to {OUTPUT_PATH.relative_to(REPO_ROOT)}")
    print(f"  raw p<{Q_LEVEL}: {n_sig_raw} ({n_sig_raw/n*100:.1f}%)")
    print(f"  BH q={Q_LEVEL}:  {n_sig_bh} ({n_sig_bh/n*100:.1f}%)")
    print("\nBreakdown by metric (BH-significant / total):")
    by_metric = out.groupby("metric").agg(
        total=("p_bh", "size"),
        bh_sig=("significant_bh_q05", "sum"),
    )
    print(by_metric.to_string())


if __name__ == "__main__":
    main()
