#!/usr/bin/env python3
"""
Braess Paradox Analysis for MAFIS Paper.

Reads braess_resilience_runs.csv + braess_perm_zone_runs.csv, computes:
  1. Braess ratios per solver × density × scenario
  2. Mann-Whitney U significance tests (fault vs baseline throughput)
  3. Degradation curves (throughput ratio vs fleet density)
  4. LaTeX table for the paper

Usage:
  python3 analysis/braess_analysis.py

Outputs (in results/):
  braess_ratios.csv        — Braess ratios with CI per (solver, density, scenario)
  braess_significance.csv  — Mann-Whitney p-values per (solver, density, scenario)
  braess_table.tex         — LaTeX table for paper (Braess ratio × scenario)
  braess_degradation.svg   — Degradation curves per solver
  braess_tp_contrast.svg   — Token Passing vs PIBT contrast plot
"""

import csv
import math
import os
import sys
from collections import defaultdict

RESULTS_DIR = os.path.join(os.path.dirname(__file__), "..", "results")

# ---------------------------------------------------------------------------
# Scenario display config
# ---------------------------------------------------------------------------

SCENARIO_ORDER = [
    "burst_20pct",
    "burst_50pct",
    "wear_medium",
    "wear_high",
    "zone_50t",
    "intermittent_80m15r",
    "perm_zone_100pct",
]

SCENARIO_LABEL = {
    "burst_20pct":          "Burst 20\\%",
    "burst_50pct":          "Burst 50\\%",
    "wear_medium":          "Wear (med.)",
    "wear_high":            "Wear (high)",
    "zone_50t":             "Zone (50t)",
    "intermittent_80m15r":  "Intermittent",
    "perm_zone_100pct":     "Perm. Zone",
}

SCENARIO_CATEGORY = {
    "burst_20pct":         "Permanent-distributed",
    "burst_50pct":         "Permanent-distributed",
    "wear_medium":         "Permanent-distributed",
    "wear_high":           "Permanent-distributed",
    "zone_50t":            "Recoverable",
    "intermittent_80m15r": "Recoverable",
    "perm_zone_100pct":    "Permanent-localized",
}

SOLVER_ORDER = ["pibt", "rhcr_pibt", "rhcr_pbs", "rhcr_priority_astar", "token_passing"]

SOLVER_LABEL = {
    "pibt":                 "PIBT",
    "rhcr_pibt":            "RHCR-PIBT",
    "rhcr_pbs":             "RHCR-PBS",
    "rhcr_priority_astar":  "RHCR-A*",
    "token_passing":        "Token Passing",
}

DENSITY_ORDER = [10, 20, 40, 80]


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_runs(*filenames):
    """Load and merge run CSVs, returning list of dicts."""
    rows = []
    for filename in filenames:
        path = os.path.join(RESULTS_DIR, filename)
        if not os.path.exists(path):
            print(f"  [skip] {filename} not found", file=sys.stderr)
            continue
        with open(path, newline="") as f:
            reader = csv.DictReader(f)
            for row in reader:
                rows.append(row)
    return rows


def parse_runs(rows):
    """
    Split rows into paired (baseline, fault) dicts keyed by
    (solver, topology, scenario, scheduler, num_agents, seed).
    Returns: dict mapping key -> {'baseline': throughput, 'fault': throughput}
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
            # Also grab survival_rate for degradation analysis
            sr = row.get("survival_rate", "")
            pairs[key]["survival_rate"] = float(sr) if sr else float("nan")
    return pairs


# ---------------------------------------------------------------------------
# Statistics helpers
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


def t_critical_95(n):
    """Two-tailed t critical value for 95% CI (df = n-1)."""
    TABLE = [
        12.706, 4.303, 3.182, 2.776, 2.571,
        2.447,  2.365, 2.306, 2.262, 2.228,
        2.201,  2.179, 2.160, 2.145, 2.131,
        2.120,  2.110, 2.101, 2.093, 2.086,
        2.080,  2.074, 2.069, 2.064, 2.060,
        2.056,  2.052, 2.048, 2.045, 2.042,
    ]
    df = n - 1
    if df <= 0:
        return float("inf")
    if df <= 30:
        return TABLE[df - 1]
    return 1.96


def ci95(xs):
    xs = [x for x in xs if not math.isnan(x)]
    n = len(xs)
    if n == 0:
        return float("nan"), float("nan")
    m = mean(xs)
    s = std(xs)
    t = t_critical_95(n)
    margin = t * s / math.sqrt(n)
    return m - margin, m + margin


def erf_approx(x):
    """Abramowitz & Stegun approximation for erf(x), max error 1.5e-7."""
    t = 1.0 / (1.0 + 0.3275911 * abs(x))
    poly = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 +
           t * (-1.453152027 + t * 1.061405429))))
    result = 1.0 - poly * math.exp(-x * x)
    return result if x >= 0 else -result


def normal_sf(z):
    """Survival function of standard normal: P(Z > z)."""
    return 0.5 * (1.0 - erf_approx(z / math.sqrt(2.0)))


def mann_whitney_u(a, b):
    """
    Two-sided Mann-Whitney U test with normal approximation.
    Returns (U, p_value). Works well for n >= 10.
    """
    a = [x for x in a if not math.isnan(x)]
    b = [x for x in b if not math.isnan(x)]
    n1, n2 = len(a), len(b)
    if n1 == 0 or n2 == 0:
        return float("nan"), float("nan")

    # Compute U1: for each a_i, count b_j < a_i (ties count 0.5)
    u1 = sum(
        sum(1.0 if bi < ai else (0.5 if bi == ai else 0.0) for bi in b)
        for ai in a
    )
    u2 = n1 * n2 - u1
    u = min(u1, u2)

    # Normal approximation (valid for n >= ~10)
    mu_u = n1 * n2 / 2.0
    sigma_u = math.sqrt(n1 * n2 * (n1 + n2 + 1) / 12.0)
    if sigma_u == 0:
        return u, 1.0

    # Continuity correction
    z = (u - mu_u - 0.5) / sigma_u
    p = 2.0 * normal_sf(abs(z))
    return u, min(p, 1.0)


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
    delta = (# concordant - # discordant) / (n1 * n2)
    Range: -1 to +1. |d| < 0.147 negligible, < 0.33 small, < 0.474 medium, >= 0.474 large.
    """
    a = [x for x in a if not math.isnan(x)]
    b = [x for x in b if not math.isnan(x)]
    n1, n2 = len(a), len(b)
    if n1 == 0 or n2 == 0:
        return float("nan")
    concordant = sum(1 for ai in a for bi in b if ai > bi)
    discordant = sum(1 for ai in a for bi in b if ai < bi)
    return (concordant - discordant) / (n1 * n2)


def benjamini_hochberg(p_values):
    """
    Benjamini-Hochberg FDR correction.
    Input: list of (key, p_value) tuples.
    Returns: dict mapping key -> adjusted p_value.
    """
    # Filter out NaN p-values
    valid = [(k, p) for k, p in p_values if not math.isnan(p)]
    if not valid:
        return {k: float("nan") for k, _ in p_values}

    # Sort by p-value
    valid.sort(key=lambda x: x[1])
    m = len(valid)

    # Compute adjusted p-values (step-up procedure)
    adjusted = {}
    prev_adj = 0.0
    for rank, (key, p) in enumerate(valid, 1):
        adj = p * m / rank
        adj = max(adj, prev_adj)  # enforce monotonicity
        adj = min(adj, 1.0)
        adjusted[key] = adj
        prev_adj = adj

    # Fill NaN entries
    for k, p in p_values:
        if k not in adjusted:
            adjusted[k] = float("nan")

    return adjusted


# ---------------------------------------------------------------------------
# Core analysis
# ---------------------------------------------------------------------------

def compute_braess_ratios(pairs):
    """
    For each (solver, num_agents, scenario), compute Braess ratios across seeds.
    Braess ratio = fault_throughput / baseline_throughput per paired run.
    Returns: dict (solver, num_agents, scenario) -> list of ratios
    """
    ratios = defaultdict(list)
    baselines = defaultdict(list)
    faults = defaultdict(list)

    for (solver, topology, scenario, scheduler, num_agents, seed), data in pairs.items():
        bl = data.get("baseline", float("nan"))
        ft = data.get("fault", float("nan"))
        if math.isnan(bl) or math.isnan(ft) or bl == 0:
            continue
        key = (solver, num_agents, scenario)
        ratios[key].append(ft / bl)
        baselines[key].append(bl)
        faults[key].append(ft)

    return ratios, baselines, faults


def format_p(p):
    """Format p-value for display."""
    if math.isnan(p):
        return "—"
    if p < 0.001:
        return "<0.001"
    if p < 0.01:
        return f"{p:.3f}*"
    if p < 0.05:
        return f"{p:.3f}*"
    return f"{p:.3f}"


def fmt_ratio(r, lo, hi):
    """Format a Braess ratio ± CI for the table."""
    if math.isnan(r):
        return "—"
    star = " \\dag" if r > 1.0 and hi > 1.0 else ""  # Both mean and CI above 1
    return f"{r:.3f} [{lo:.3f}, {hi:.3f}]{star}"


# ---------------------------------------------------------------------------
# Output: CSV tables
# ---------------------------------------------------------------------------

def write_ratios_csv(ratios, baselines, faults, adjusted_p):
    path = os.path.join(RESULTS_DIR, "braess_ratios.csv")
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow([
            "solver", "num_agents", "scenario", "n_seeds",
            "ratio_mean", "ratio_ci95_lo", "ratio_ci95_hi",
            "baseline_mean", "fault_mean",
            "mw_u", "mw_p_raw", "mw_p_adj", "effect_r", "cliffs_d",
        ])
        for solver in SOLVER_ORDER:
            for density in DENSITY_ORDER:
                for scenario in SCENARIO_ORDER:
                    key = (solver, density, scenario)
                    rs = ratios.get(key, [])
                    if not rs:
                        continue
                    bl = baselines.get(key, [])
                    ft = faults.get(key, [])
                    n = len(rs)
                    r_mean = mean(rs)
                    r_lo, r_hi = ci95(rs)
                    bl_mean = mean(bl)
                    ft_mean = mean(ft)
                    u, p = mann_whitney_u(ft, bl)
                    er = effect_size_r(u, len(ft), len(bl))
                    cd = cliffs_delta(ft, bl)
                    p_adj = adjusted_p.get(key, float("nan"))
                    w.writerow([
                        solver, density, scenario, n,
                        f"{r_mean:.4f}", f"{r_lo:.4f}", f"{r_hi:.4f}",
                        f"{bl_mean:.4f}", f"{ft_mean:.4f}",
                        f"{u:.1f}" if not math.isnan(u) else "",
                        f"{p:.4f}" if not math.isnan(p) else "",
                        f"{p_adj:.4f}" if not math.isnan(p_adj) else "",
                        f"{er:.3f}" if not math.isnan(er) else "",
                        f"{cd:.3f}" if not math.isnan(cd) else "",
                    ])
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: LaTeX table
# ---------------------------------------------------------------------------

def write_latex_table(ratios, baselines, faults, adjusted_p):
    """
    Table: Braess ratio (fault_throughput / baseline_throughput) per solver × density.
    One subtable per scenario.
    Confirmed Braess = CI lower > 1 AND BH-adjusted p < 0.05.
    """
    path = os.path.join(RESULTS_DIR, "braess_table.tex")
    lines = []

    lines.append("% Braess resilience table — auto-generated by analysis/braess_analysis.py")
    lines.append("% Columns: PIBT | RHCR-PIBT | RHCR-PBS | RHCR-A* | Token Passing")
    lines.append("% Rows: fleet density (10, 20, 40, 80 agents)")
    lines.append("% p-values: BH-FDR corrected across all 140 tests")
    lines.append("% \\dag = confirmed Braess (CI lower > 1 AND adj. p < 0.05)")
    lines.append("")

    for scenario in SCENARIO_ORDER:
        label = SCENARIO_LABEL.get(scenario, scenario)
        cat = SCENARIO_CATEGORY.get(scenario, "")
        col_spec = "r" + "c" * len(SOLVER_ORDER)
        lines.append(f"% --- {label} ({cat}) ---")
        lines.append(f"\\begin{{table}}[ht]")
        lines.append(f"  \\caption{{Braess ratios under \\textbf{{{label}}} fault scenario.}}")
        lines.append(f"  \\label{{tab:braess:{scenario}}}")
        lines.append(f"  \\centering")
        lines.append(f"  \\begin{{tabular}}{{{col_spec}}}")
        lines.append(f"    \\toprule")

        # Header row
        solver_labels = " & ".join(
            f"\\textbf{{{SOLVER_LABEL[s]}}}" for s in SOLVER_ORDER
        )
        lines.append(f"    $n$ & {solver_labels} \\\\")
        lines.append(f"    \\midrule")

        for density in DENSITY_ORDER:
            cells = []
            for solver in SOLVER_ORDER:
                key = (solver, density, scenario)
                rs = ratios.get(key, [])
                bl = baselines.get(key, [])
                ft = faults.get(key, [])
                if not rs:
                    cells.append("—")
                    continue
                r_mean = mean(rs)
                r_lo, r_hi = ci95(rs)
                p_adj = adjusted_p.get(key, 1.0)
                sig = p_adj < 0.05 and not math.isnan(p_adj)

                if r_mean > 1.0 and r_lo > 1.0 and sig:
                    # Confirmed: CI lower > 1 AND adjusted p < 0.05
                    cells.append(f"$\\mathbf{{{r_mean:.3f}}}^{{\\dag}}$")
                elif sig:
                    cells.append(f"${r_mean:.3f}^{{*}}$")
                else:
                    cells.append(f"${r_mean:.3f}$")

            lines.append(f"    {density} & " + " & ".join(cells) + " \\\\")

        lines.append(f"    \\bottomrule")
        lines.append(f"    \\multicolumn{{{len(SOLVER_ORDER)+1}}}{{l}}{{")
        lines.append(f"      \\footnotesize $\\dag$ confirmed Braess (CI lower $>$ 1 AND BH-adjusted $p < 0.05$); "
                     f"$^*$ BH-adjusted $p < 0.05$.}}")
        lines.append(f"  \\end{{tabular}}")
        lines.append(f"\\end{{table}}")
        lines.append("")

    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: Significance table CSV
# ---------------------------------------------------------------------------

def write_significance_csv(baselines, faults):
    path = os.path.join(RESULTS_DIR, "braess_significance.csv")
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        header = ["solver", "num_agents"] + SCENARIO_ORDER
        w.writerow(header)
        for solver in SOLVER_ORDER:
            for density in DENSITY_ORDER:
                row = [solver, density]
                for scenario in SCENARIO_ORDER:
                    key = (solver, density, scenario)
                    bl = baselines.get(key, [])
                    ft = faults.get(key, [])
                    if not bl or not ft:
                        row.append("")
                        continue
                    _, p = mann_whitney_u(ft, bl)
                    row.append(format_p(p))
                w.writerow(row)
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: SVG degradation curves
# ---------------------------------------------------------------------------

def _svg_path(points, x_scale, y_scale, x_off, y_off, height):
    """Convert (x, y) data points to SVG path d attribute."""
    parts = []
    for i, (x, y) in enumerate(points):
        sx = x_off + x * x_scale
        sy = y_off + height - y * y_scale
        cmd = "M" if i == 0 else "L"
        parts.append(f"{cmd}{sx:.1f},{sy:.1f}")
    return " ".join(parts)


SOLVER_COLORS = {
    "pibt":                "#e07b39",  # amber
    "rhcr_pibt":           "#4a9eca",  # steel blue
    "rhcr_pbs":            "#5cb85c",  # green
    "rhcr_priority_astar": "#9b59b6",  # purple
    "token_passing":       "#e74c3c",  # red
}


def write_degradation_svg(ratios):
    """
    One SVG per scenario category showing Braess ratio vs density per solver.
    x = density (10, 20, 40, 80 → normalized 0..1)
    y = mean Braess ratio (fault/baseline throughput)
    """
    # One combined SVG: 3 columns (one per category) × 2 rows (scenarios within category)
    W, H = 900, 360  # total canvas
    margin = {"top": 40, "right": 30, "bottom": 50, "left": 55}
    n_cols = 3
    panel_w = (W - margin["left"] - margin["right"]) // n_cols
    panel_h = H - margin["top"] - margin["bottom"]

    categories = ["Recoverable", "Permanent-distributed", "Permanent-localized"]
    cat_scenarios = {cat: [] for cat in categories}
    for sc in SCENARIO_ORDER:
        cat = SCENARIO_CATEGORY.get(sc)
        if cat:
            cat_scenarios[cat].append(sc)

    # Determine y range across all data
    all_means = []
    for key, rs in ratios.items():
        if rs:
            all_means.append(mean(rs))
    y_min = max(0.0, min(all_means) - 0.05) if all_means else 0.0
    y_max = max(all_means) + 0.1 if all_means else 1.5
    y_min = min(y_min, 0.9)
    y_max = max(y_max, 1.1)

    # Density positions (0, 1, 2, 3)
    density_idx = {d: i for i, d in enumerate(DENSITY_ORDER)}

    svg_lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">',
        f'  <rect width="{W}" height="{H}" fill="#1a1a2e"/>',
        f'  <style>',
        f'    text {{ font-family: monospace; fill: #c8c8d4; }}',
        f'    .axis-line {{ stroke: #444; stroke-width: 1; }}',
        f'    .grid-line {{ stroke: #333; stroke-width: 1; stroke-dasharray: 3,3; }}',
        f'    .baseline {{ stroke: #555; stroke-width: 1; stroke-dasharray: 6,3; }}',
        f'  </style>',
    ]

    for col, cat in enumerate(categories):
        scenarios_in_cat = cat_scenarios[cat]
        x0 = margin["left"] + col * panel_w
        y0 = margin["top"]
        pw = panel_w - 10
        ph = panel_h

        x_scale = pw / 3.0   # 4 density levels → 3 intervals
        y_range = y_max - y_min
        y_scale = ph / y_range if y_range > 0 else ph

        # Panel background
        svg_lines.append(f'  <rect x="{x0}" y="{y0}" width="{pw}" height="{ph}" fill="#12122a" rx="2"/>')

        # Category label
        svg_lines.append(f'  <text x="{x0 + pw//2}" y="{y0 - 10}" text-anchor="middle" font-size="11" fill="#a0a0c0">{cat}</text>')

        # Baseline ratio = 1.0 line
        y1 = y0 + ph - (1.0 - y_min) * y_scale
        svg_lines.append(f'  <line x1="{x0}" y1="{y1:.1f}" x2="{x0+pw}" y2="{y1:.1f}" class="baseline"/>')

        # Grid lines + y axis labels
        for tick in [0.6, 0.8, 1.0, 1.2, 1.4]:
            if y_min <= tick <= y_max:
                ty = y0 + ph - (tick - y_min) * y_scale
                svg_lines.append(f'  <line x1="{x0}" y1="{ty:.1f}" x2="{x0+pw}" y2="{ty:.1f}" class="grid-line"/>')
                if col == 0:
                    svg_lines.append(f'  <text x="{x0-4}" y="{ty+4:.1f}" text-anchor="end" font-size="9">{tick:.1f}</text>')

        # x axis labels (densities)
        for i, d in enumerate(DENSITY_ORDER):
            tx = x0 + i * x_scale
            svg_lines.append(f'  <text x="{tx:.1f}" y="{y0+ph+14}" text-anchor="middle" font-size="9">{d}</text>')

        # Axis lines
        svg_lines.append(f'  <line x1="{x0}" y1="{y0}" x2="{x0}" y2="{y0+ph}" class="axis-line"/>')
        svg_lines.append(f'  <line x1="{x0}" y1="{y0+ph}" x2="{x0+pw}" y2="{y0+ph}" class="axis-line"/>')

        # Curves: one per solver, averaged over scenarios in this category
        for solver in SOLVER_ORDER:
            color = SOLVER_COLORS[solver]
            points = []
            for i, density in enumerate(DENSITY_ORDER):
                vals = []
                for scenario in scenarios_in_cat:
                    key = (solver, density, scenario)
                    rs = ratios.get(key, [])
                    if rs:
                        vals.append(mean(rs))
                if vals:
                    avg = sum(vals) / len(vals)
                    x = x0 + i * x_scale
                    y = y0 + ph - (avg - y_min) * y_scale
                    points.append((x, y))

            if len(points) >= 2:
                d_attr = "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in points)
                svg_lines.append(f'  <path d="{d_attr}" stroke="{color}" stroke-width="2" fill="none" opacity="0.85"/>')
                # Dots
                for x, y in points:
                    svg_lines.append(f'  <circle cx="{x:.1f}" cy="{y:.1f}" r="3" fill="{color}"/>')

    # Legend (bottom center)
    lx = margin["left"]
    ly = H - 14
    for solver in SOLVER_ORDER:
        color = SOLVER_COLORS[solver]
        label = SOLVER_LABEL[solver]
        svg_lines.append(f'  <line x1="{lx}" y1="{ly-4}" x2="{lx+16}" y2="{ly-4}" stroke="{color}" stroke-width="2"/>')
        svg_lines.append(f'  <text x="{lx+20}" y="{ly}" font-size="9">{label}</text>')
        lx += 110

    # y-axis title
    svg_lines.append(f'  <text transform="rotate(-90)" x="{-(H//2)}" y="12" text-anchor="middle" font-size="10">Braess Ratio (fault/baseline throughput)</text>')
    # x-axis title
    svg_lines.append(f'  <text x="{W//2}" y="{H-2}" text-anchor="middle" font-size="10">Fleet Size (agents)</text>')

    svg_lines.append("</svg>")

    path = os.path.join(RESULTS_DIR, "braess_degradation.svg")
    with open(path, "w") as f:
        f.write("\n".join(svg_lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Console summary
# ---------------------------------------------------------------------------

def print_summary(ratios, baselines, faults, adjusted_p):
    print("\n" + "="*90)
    print("BRAESS PARADOX ANALYSIS — MAFIS Paper (with BH-FDR correction)")
    print("="*90)
    print(f"{'Solver':<20} {'n':>3} {'Scenario':<25} {'Ratio':>7} {'95% CI':>18} {'p_raw':>8} {'p_adj':>8} {'Cliff':>6}")
    print("-"*90)

    confirmed_braess = 0
    sig_raw_only = 0
    total_tests = 0

    for solver in SOLVER_ORDER:
        for density in DENSITY_ORDER:
            for scenario in SCENARIO_ORDER:
                key = (solver, density, scenario)
                rs = ratios.get(key, [])
                if not rs:
                    continue
                total_tests += 1
                bl = baselines.get(key, [])
                ft = faults.get(key, [])
                r = mean(rs)
                lo, hi = ci95(rs)
                _, p_raw = mann_whitney_u(ft, bl)
                p_adj = adjusted_p.get(key, float("nan"))
                cd = cliffs_delta(ft, bl)
                sig_adj = p_adj < 0.05 and not math.isnan(p_adj)
                sig_raw = p_raw < 0.05 and not math.isnan(p_raw)

                if sig_raw and not sig_adj:
                    sig_raw_only += 1

                braess = ""
                if r > 1.0 and lo > 1.0 and sig_adj:
                    braess = " ←CONFIRMED"
                    confirmed_braess += 1
                elif r > 1.0 and lo > 1.0:
                    braess = " (CI only)"

                flag = "**" if sig_adj else ("* " if sig_raw else "  ")
                cd_label = f"{cd:>6.3f}" if not math.isnan(cd) else "   —  "
                print(f"{SOLVER_LABEL[solver]:<20} {density:>3} {scenario:<25} {r:>7.3f} [{lo:.3f}, {hi:.3f}] {format_p(p_raw):>8} {format_p(p_adj):>8}{flag} {cd_label}{braess}")

    print(f"\n--- Summary: {total_tests} tests, {confirmed_braess} confirmed Braess (CI>1 + adj.p<0.05)")
    print(f"    {sig_raw_only} results lost significance after FDR correction")

    print("\n--- Braess threshold per solver (CI > 1 AND BH-adjusted p < 0.05) ---")
    for solver in SOLVER_ORDER:
        threshold = None
        for density in DENSITY_ORDER:
            permanent_dist = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high"]
            for sc in permanent_dist:
                key = (solver, density, sc)
                rs = ratios.get(key, [])
                if not rs:
                    continue
                r = mean(rs)
                lo, _ = ci95(rs)
                p_adj = adjusted_p.get(key, 1.0)
                if r > 1.0 and lo > 1.0 and p_adj < 0.05 and threshold is None:
                    threshold = (density, sc)
            if threshold:
                break
        if threshold:
            print(f"  {SOLVER_LABEL[solver]:<20} → n={threshold[0]} ({threshold[1]})")
        else:
            print(f"  {SOLVER_LABEL[solver]:<20} → No confirmed Braess benefit")

    print("\n--- Token Passing vulnerability (permanent vs recoverable) ---")
    for density in DENSITY_ORDER:
        perm = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high", "perm_zone_100pct"]
        rec  = ["zone_50t", "intermittent_80m15r"]
        tp_perm = []
        tp_rec  = []
        for sc in perm:
            tp_perm.extend(ratios.get(("token_passing", density, sc), []))
        for sc in rec:
            tp_rec.extend(ratios.get(("token_passing", density, sc), []))
        if tp_perm and tp_rec:
            print(f"  n={density:>2}: perm={mean(tp_perm):.3f}  recoverable={mean(tp_rec):.3f}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    print("Loading data...")
    rows = load_runs(
        "braess_resilience_runs.csv",
        "braess_perm_zone_runs.csv",
    )
    print(f"  {len(rows)} total rows loaded")

    pairs = parse_runs(rows)
    print(f"  {len(pairs)} paired runs")

    ratios, baselines, faults = compute_braess_ratios(pairs)
    print(f"  {len(ratios)} (solver, density, scenario) groups")

    # Compute all raw p-values for BH-FDR correction
    print("\nComputing BH-FDR adjusted p-values across all 140 tests...")
    raw_p_list = []
    for solver in SOLVER_ORDER:
        for density in DENSITY_ORDER:
            for scenario in SCENARIO_ORDER:
                key = (solver, density, scenario)
                bl = baselines.get(key, [])
                ft = faults.get(key, [])
                if bl and ft:
                    _, p = mann_whitney_u(ft, bl)
                    raw_p_list.append((key, p))
                else:
                    raw_p_list.append((key, float("nan")))

    adjusted_p = benjamini_hochberg(raw_p_list)
    n_sig_raw = sum(1 for _, p in raw_p_list if not math.isnan(p) and p < 0.05)
    n_sig_adj = sum(1 for p in adjusted_p.values() if not math.isnan(p) and p < 0.05)
    print(f"  Raw significant (p<0.05): {n_sig_raw}")
    print(f"  BH-adjusted significant: {n_sig_adj}")
    print(f"  Lost after correction: {n_sig_raw - n_sig_adj}")

    print("\nWriting outputs...")
    write_ratios_csv(ratios, baselines, faults, adjusted_p)
    write_significance_csv(baselines, faults)
    write_latex_table(ratios, baselines, faults, adjusted_p)
    write_degradation_svg(ratios)

    print_summary(ratios, baselines, faults, adjusted_p)


if __name__ == "__main__":
    main()
