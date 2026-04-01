"""
stats.py — Shared statistical helpers for MAFIS experiment analysis scripts.

Pure Python stdlib only. No numpy/scipy/matplotlib dependencies.
Imported by all analysis scripts in this directory.
"""
import csv
import math
import os
import sys
from collections import defaultdict

RESULTS_DIR = os.path.join(os.path.dirname(__file__), "..", "results")


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_runs(*filenames):
    """Load and merge run CSVs from results/, returning list of row dicts."""
    rows = []
    for filename in filenames:
        path = os.path.join(RESULTS_DIR, filename)
        if not os.path.exists(path):
            print(f"  [skip] {filename} not found", file=sys.stderr)
            continue
        with open(path, newline="") as f:
            reader = csv.DictReader(f)
            rows.extend(reader)
    return rows


def pair_runs(rows):
    """
    Split rows into paired (baseline, fault) dicts.

    Key: (solver, topology, scenario, scheduler, num_agents, seed).
    Returns dict mapping key -> {'baseline': float, 'fault': float, 'survival_rate': float}.
    """
    pairs = defaultdict(dict)
    for row in rows:
        key = (
            row["solver"],
            row["topology"],
            row["scenario"],
            row["scheduler"],
            int(row["num_agents"]),
            int(row["seed"]),
        )
        tp = float(row["avg_throughput"]) if row["avg_throughput"] else float("nan")
        is_baseline = row["is_baseline"].strip().lower() == "true"
        if is_baseline:
            pairs[key]["baseline"] = tp
        else:
            pairs[key]["fault"] = tp
            sr = row.get("survival_rate", "")
            pairs[key]["survival_rate"] = float(sr) if sr else float("nan")
    return pairs


# ---------------------------------------------------------------------------
# Descriptive statistics
# ---------------------------------------------------------------------------

def mean(xs):
    xs = [x for x in xs if not math.isnan(x)]
    return sum(xs) / len(xs) if xs else float("nan")


def std(xs, ddof=1):
    xs = [x for x in xs if not math.isnan(x)]
    n = len(xs)
    if n < 2:
        return 0.0
    m = sum(xs) / n
    return math.sqrt(sum((x - m) ** 2 for x in xs) / (n - ddof))


_T95_TABLE = [
    12.706, 4.303, 3.182, 2.776, 2.571,
    2.447,  2.365, 2.306, 2.262, 2.228,
    2.201,  2.179, 2.160, 2.145, 2.131,
    2.120,  2.110, 2.101, 2.093, 2.086,
    2.080,  2.074, 2.069, 2.064, 2.060,
    2.056,  2.052, 2.048, 2.045, 2.042,
]


def t_critical_95(n):
    """Two-tailed t critical value for 95% CI (df = n-1). Falls back to 1.96 for n > 31."""
    df = n - 1
    if df <= 0:
        return float("inf")
    if df <= 30:
        return _T95_TABLE[df - 1]
    return 1.96


def ci95(xs):
    """95% CI for the mean. Returns (lo, hi)."""
    xs = [x for x in xs if not math.isnan(x)]
    n = len(xs)
    if n == 0:
        return float("nan"), float("nan")
    m = mean(xs)
    s = std(xs)
    margin = t_critical_95(n) * s / math.sqrt(n)
    return m - margin, m + margin


# ---------------------------------------------------------------------------
# Hypothesis testing
# ---------------------------------------------------------------------------

def _erf_approx(x):
    """Abramowitz & Stegun approximation for erf(x), max error 1.5e-7."""
    t = 1.0 / (1.0 + 0.3275911 * abs(x))
    p = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 +
        t * (-1.453152027 + t * 1.061405429))))
    result = 1.0 - p * math.exp(-x * x)
    return result if x >= 0 else -result


def _normal_sf(z):
    """Survival function of standard normal: P(Z > z)."""
    return 0.5 * (1.0 - _erf_approx(z / math.sqrt(2.0)))


def mann_whitney_u(a, b):
    """
    Two-sided Mann-Whitney U test with normal approximation.
    Returns (U, p_value). Valid for n >= ~10; uses continuity correction.
    """
    a = [x for x in a if not math.isnan(x)]
    b = [x for x in b if not math.isnan(x)]
    n1, n2 = len(a), len(b)
    if n1 == 0 or n2 == 0:
        return float("nan"), float("nan")
    u1 = sum(
        sum(1.0 if bi < ai else (0.5 if bi == ai else 0.0) for bi in b)
        for ai in a
    )
    u2 = n1 * n2 - u1
    u = min(u1, u2)
    mu_u = n1 * n2 / 2.0
    sigma_u = math.sqrt(n1 * n2 * (n1 + n2 + 1) / 12.0)
    if sigma_u == 0:
        return u, 1.0
    z = (u - mu_u - 0.5) / sigma_u  # continuity correction
    return u, min(2.0 * _normal_sf(abs(z)), 1.0)


def effect_size_r(u, n1, n2):
    """Effect size r = Z / sqrt(n1 + n2)."""
    if n1 == 0 or n2 == 0:
        return float("nan")
    mu_u = n1 * n2 / 2.0
    sigma_u = math.sqrt(n1 * n2 * (n1 + n2 + 1) / 12.0)
    if sigma_u == 0:
        return 0.0
    z = (u - mu_u) / sigma_u
    return abs(z) / math.sqrt(n1 + n2)


def cliffs_delta(a, b):
    """
    Cliff's delta: non-parametric effect size.
    Range -1..+1. |d| < 0.147 negligible, < 0.33 small, < 0.474 medium, >= 0.474 large.
    Positive = a tends to be larger than b.
    """
    a = [x for x in a if not math.isnan(x)]
    b = [x for x in b if not math.isnan(x)]
    n1, n2 = len(a), len(b)
    if n1 == 0 or n2 == 0:
        return float("nan")
    conc = sum(1 for ai in a for bi in b if ai > bi)
    disc = sum(1 for ai in a for bi in b if ai < bi)
    return (conc - disc) / (n1 * n2)


def benjamini_hochberg(p_values):
    """
    Benjamini-Hochberg FDR correction.
    Input: list of (key, p_value) tuples.
    Returns: dict key -> adjusted p-value.
    """
    valid = [(k, p) for k, p in p_values if not math.isnan(p)]
    if not valid:
        return {k: float("nan") for k, _ in p_values}
    valid.sort(key=lambda x: x[1])
    m = len(valid)
    adjusted = {}
    prev_adj = 0.0
    for rank, (key, p) in enumerate(valid, 1):
        adj = min(max(p * m / rank, prev_adj), 1.0)
        adjusted[key] = adj
        prev_adj = adj
    for k, _ in p_values:
        if k not in adjusted:
            adjusted[k] = float("nan")
    return adjusted


# ---------------------------------------------------------------------------
# Formatting
# ---------------------------------------------------------------------------

def format_p(p):
    """Format p-value for console display."""
    if math.isnan(p):
        return "—"
    if p < 0.001:
        return "<0.001"
    if p < 0.05:
        return f"{p:.3f}*"
    return f"{p:.3f}"
