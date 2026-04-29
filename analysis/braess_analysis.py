#!/usr/bin/env python3
"""
Braess Paradox Analysis for MAFIS.

RQ5: Does fault type interact with fleet density and solver architecture?

Tests the Braess hypothesis: under congestion, permanent agent removal can
paradoxically improve throughput for reactive solvers (FT > 1).

Reads results/braess_resilience_runs.csv + results/braess_perm_zone_runs.csv,
computes:
  1. Braess ratios per solver × density × scenario
  2. Mann-Whitney U significance tests (BH-FDR corrected across all tests)
  3. Degradation curves (FT ratio vs fleet density per solver)
  4. LaTeX table for the paper

Usage:
  python3 analysis/braess_analysis.py

Outputs (in results/):
  braess_ratios.csv        -- Braess ratios with CI per (solver, density, scenario)
  braess_significance.csv  -- Mann-Whitney p-values per (solver, density, scenario)
  braess_table.tex         -- LaTeX table for paper
  braess_degradation.svg   -- Degradation curves per solver
"""

import csv
import math
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from stats import (
    load_runs, pair_runs, mean, ci95, mann_whitney_u, effect_size_r,
    cliffs_delta, benjamini_hochberg, format_p, RESULTS_DIR,
)
from constants import (
    SCENARIO_ORDER, SCENARIO_LABEL, SCENARIO_CATEGORY,
    SOLVER_ORDER, SOLVER_LABEL, SOLVER_COLORS, DENSITY_ORDER,
)


# ---------------------------------------------------------------------------
# Braess ratio computation
# ---------------------------------------------------------------------------

def compute_braess_ratios(pairs):
    """
    Group by (solver, num_agents, scenario).
    Braess ratio = fault_throughput / baseline_throughput per paired run.
    Returns: ratios, baselines, faults — all dicts keyed by (solver, density, scenario).
    """
    from collections import defaultdict
    ratios   = defaultdict(list)
    baselines = defaultdict(list)
    faults   = defaultdict(list)
    for (solver, topology, scenario, scheduler, num_agents, seed), data in pairs.items():
        bl = data.get("baseline", float("nan"))
        ft = data.get("fault",    float("nan"))
        if math.isnan(bl) or math.isnan(ft) or bl == 0:
            continue
        key = (solver, num_agents, scenario)
        ratios[key].append(ft / bl)
        baselines[key].append(bl)
        faults[key].append(ft)
    return ratios, baselines, faults


# ---------------------------------------------------------------------------
# Formatting helpers
# ---------------------------------------------------------------------------

def fmt_ratio(r, lo, hi):
    """Format a Braess ratio ± CI for the table."""
    if math.isnan(r):
        return "—"
    dagger = " \\dag" if r > 1.0 and lo > 1.0 else ""
    return f"{r:.3f} [{lo:.3f}, {hi:.3f}]{dagger}"


# ---------------------------------------------------------------------------
# Output: ratios CSV
# ---------------------------------------------------------------------------

def write_ratios_csv(ratios, baselines, faults, adjusted_p):
    path = os.path.join(RESULTS_DIR, "braess_ratios.csv")
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in ratios)]
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow([
            "solver", "num_agents", "scenario", "n_seeds",
            "ratio_mean", "ratio_ci95_lo", "ratio_ci95_hi",
            "baseline_mean", "fault_mean",
            "mw_u", "mw_p_raw", "mw_p_adj", "effect_r", "cliffs_d",
        ])
        for solver in solvers:
            for density in DENSITY_ORDER:
                for scenario in SCENARIO_ORDER:
                    key = (solver, density, scenario)
                    rs  = ratios.get(key, [])
                    if not rs:
                        continue
                    bl = baselines.get(key, [])
                    ft = faults.get(key, [])
                    r_mean = mean(rs)
                    r_lo, r_hi = ci95(rs)
                    u, p = mann_whitney_u(ft, bl)
                    er = effect_size_r(u, len(ft), len(bl))
                    cd = cliffs_delta(ft, bl)
                    p_adj = adjusted_p.get(key, float("nan"))
                    w.writerow([
                        solver, density, scenario, len(rs),
                        f"{r_mean:.4f}", f"{r_lo:.4f}", f"{r_hi:.4f}",
                        f"{mean(bl):.4f}", f"{mean(ft):.4f}",
                        f"{u:.1f}" if not math.isnan(u) else "",
                        f"{p:.4f}" if not math.isnan(p) else "",
                        f"{p_adj:.4f}" if not math.isnan(p_adj) else "",
                        f"{er:.3f}" if not math.isnan(er) else "",
                        f"{cd:.3f}" if not math.isnan(cd) else "",
                    ])
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: significance CSV
# ---------------------------------------------------------------------------

def write_significance_csv(baselines, faults):
    path = os.path.join(RESULTS_DIR, "braess_significance.csv")
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in baselines)]
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["solver", "num_agents"] + SCENARIO_ORDER)
        for solver in solvers:
            for density in DENSITY_ORDER:
                row = [solver, density]
                for scenario in SCENARIO_ORDER:
                    key = (solver, density, scenario)
                    bl  = baselines.get(key, [])
                    ft  = faults.get(key, [])
                    if bl and ft:
                        _, p = mann_whitney_u(ft, bl)
                        row.append(format_p(p))
                    else:
                        row.append("")
                w.writerow(row)
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: LaTeX table
# ---------------------------------------------------------------------------

def write_latex_table(ratios, baselines, faults, adjusted_p):
    """
    One table per scenario. Confirmed Braess = CI lower > 1 AND BH-adj. p < 0.05.
    """
    path = os.path.join(RESULTS_DIR, "braess_table.tex")
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in ratios)]
    lines = [
        "% Braess resilience table — auto-generated by analysis/braess_analysis.py",
        f"% Columns: {', '.join(SOLVER_LABEL.get(s, s) for s in solvers)}",
        "% Rows: fleet density (10, 20, 40, 80 agents)",
        "% p-values: BH-FDR corrected across all tests",
        "% \\dag = confirmed Braess (CI lower > 1 AND BH-adj. p < 0.05)",
        "",
    ]

    for scenario in SCENARIO_ORDER:
        label = SCENARIO_LABEL.get(scenario, scenario)
        cat   = SCENARIO_CATEGORY.get(scenario, "")
        col_spec = "r" + "c" * len(solvers)
        lines += [
            f"% --- {label} ({cat}) ---",
            f"\\begin{{table}}[ht]",
            f"  \\caption{{Braess ratios under \\textbf{{{label}}} fault scenario.}}",
            f"  \\label{{tab:braess:{scenario}}}",
            f"  \\centering",
            f"  \\begin{{tabular}}{{{col_spec}}}",
            f"    \\toprule",
        ]
        hdr = " & ".join(f"\\textbf{{{SOLVER_LABEL.get(s, s)}}}" for s in solvers)
        lines.append(f"    $n$ & {hdr} \\\\")
        lines.append(f"    \\midrule")

        for density in DENSITY_ORDER:
            cells = []
            for solver in solvers:
                key = (solver, density, scenario)
                rs  = ratios.get(key, [])
                if not rs:
                    cells.append("—")
                    continue
                r_mean = mean(rs)
                r_lo, _ = ci95(rs)
                p_adj = adjusted_p.get(key, 1.0)
                sig   = not math.isnan(p_adj) and p_adj < 0.05

                if r_mean > 1.0 and r_lo > 1.0 and sig:
                    cells.append(f"$\\mathbf{{{r_mean:.3f}}}^{{\\dag}}$")
                elif sig:
                    cells.append(f"${r_mean:.3f}^{{*}}$")
                else:
                    cells.append(f"${r_mean:.3f}$")
            lines.append(f"    {density} & " + " & ".join(cells) + " \\\\")

        lines += [
            f"    \\bottomrule",
            f"    \\multicolumn{{{len(solvers)+1}}}{{l}}{{",
            f"      \\footnotesize $\\dag$ confirmed Braess (CI lower $>$ 1, BH-adj.~$p < 0.05$); "
            f"$^*$ BH-adj.~$p < 0.05$.}}",
            f"  \\end{{tabular}}",
            f"\\end{{table}}",
            "",
        ]

    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: SVG degradation curves
# ---------------------------------------------------------------------------

def write_degradation_svg(ratios):
    """
    Three-panel SVG: one column per scenario category, curves per solver.
    x = fleet density, y = mean Braess ratio.
    """
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in ratios)]

    W, H   = 900, 360
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

    all_means = [mean(rs) for rs in ratios.values() if rs]
    y_min = max(0.0, min(all_means) - 0.05) if all_means else 0.0
    y_max = max(all_means) + 0.10 if all_means else 1.5
    y_min = min(y_min, 0.9)
    y_max = max(y_max, 1.1)

    d_idx = {d: i for i, d in enumerate(DENSITY_ORDER)}

    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">',
        f'  <rect width="{W}" height="{H}" fill="#1a1a2e"/>',
        f'  <style>',
        f'    text      {{ font-family: monospace; fill: #c8c8d4; }}',
        f'    .axis     {{ stroke: #444; stroke-width: 1; }}',
        f'    .grid     {{ stroke: #333; stroke-width: 1; stroke-dasharray: 3,3; }}',
        f'    .baseline {{ stroke: #555; stroke-width: 1; stroke-dasharray: 6,3; }}',
        f'  </style>',
    ]

    for col, cat in enumerate(categories):
        sc_in_cat = cat_scenarios[cat]
        x0  = margin["left"] + col * panel_w
        y0  = margin["top"]
        pw  = panel_w - 10
        ph  = panel_h
        y_range = y_max - y_min
        y_scale = ph / y_range if y_range else ph
        x_scale = pw / 3.0     # 4 points → 3 intervals

        svg.append(f'  <rect x="{x0}" y="{y0}" width="{pw}" height="{ph}" fill="#12122a" rx="2"/>')
        svg.append(f'  <text x="{x0+pw//2}" y="{y0-10}" text-anchor="middle" font-size="11" fill="#a0a0c0">{cat}</text>')

        # Baseline ratio = 1.0 line
        y1 = y0 + ph - (1.0 - y_min) * y_scale
        svg.append(f'  <line x1="{x0}" y1="{y1:.1f}" x2="{x0+pw}" y2="{y1:.1f}" class="baseline"/>')

        # Grid lines + y axis labels
        for tick in [0.6, 0.8, 1.0, 1.2, 1.4]:
            if y_min <= tick <= y_max:
                ty = y0 + ph - (tick - y_min) * y_scale
                svg.append(f'  <line x1="{x0}" y1="{ty:.1f}" x2="{x0+pw}" y2="{ty:.1f}" class="grid"/>')
                if col == 0:
                    svg.append(f'  <text x="{x0-4}" y="{ty+4:.1f}" text-anchor="end" font-size="9">{tick:.1f}</text>')

        # x labels
        for d in DENSITY_ORDER:
            tx = x0 + d_idx[d] * x_scale
            svg.append(f'  <text x="{tx:.1f}" y="{y0+ph+14}" text-anchor="middle" font-size="9">{d}</text>')

        # Axis lines
        svg.append(f'  <line x1="{x0}" y1="{y0}" x2="{x0}" y2="{y0+ph}" class="axis"/>')
        svg.append(f'  <line x1="{x0}" y1="{y0+ph}" x2="{x0+pw}" y2="{y0+ph}" class="axis"/>')

        # Solver curves — averaged across scenarios in this category
        for solver in solvers:
            color  = SOLVER_COLORS.get(solver, "#888")
            points = []
            for i, density in enumerate(DENSITY_ORDER):
                vals = [mean(ratios.get((solver, density, sc), []))
                        for sc in sc_in_cat if ratios.get((solver, density, sc))]
                if vals:
                    avg = sum(v for v in vals if not math.isnan(v)) / sum(1 for v in vals if not math.isnan(v))
                    x   = x0 + i * x_scale
                    y   = y0 + ph - (avg - y_min) * y_scale
                    points.append((x, y))
            if len(points) >= 2:
                d_attr = "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in points)
                svg.append(f'  <path d="{d_attr}" stroke="{color}" stroke-width="2" fill="none" opacity="0.85"/>')
            for x, y in points:
                svg.append(f'  <circle cx="{x:.1f}" cy="{y:.1f}" r="3" fill="{color}"/>')

    # Legend (bottom)
    lx, ly = margin["left"], H - 14
    for solver in solvers:
        color = SOLVER_COLORS.get(solver, "#888")
        label = SOLVER_LABEL.get(solver, solver)
        svg.append(f'  <line x1="{lx}" y1="{ly-4}" x2="{lx+16}" y2="{ly-4}" stroke="{color}" stroke-width="2"/>')
        svg.append(f'  <text x="{lx+20}" y="{ly}" font-size="9">{label}</text>')
        lx += 115

    # Axis titles
    svg.append(
        f'  <text transform="rotate(-90)" x="{-(H//2)}" y="12" '
        f'text-anchor="middle" font-size="10">Braess Ratio (fault / baseline throughput)</text>'
    )
    svg.append(f'  <text x="{W//2}" y="{H-2}" text-anchor="middle" font-size="10">Fleet Size (agents)</text>')

    svg.append("</svg>")

    path = os.path.join(RESULTS_DIR, "braess_degradation.svg")
    with open(path, "w") as f:
        f.write("\n".join(svg))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Console summary
# ---------------------------------------------------------------------------

def print_summary(ratios, baselines, faults, adjusted_p):
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in ratios)]

    print("\n" + "=" * 90)
    print("BRAESS PARADOX ANALYSIS — RQ5 (with BH-FDR correction)")
    print("=" * 90)
    print(f"{'Solver':<20} {'n':>3} {'Scenario':<25} {'Ratio':>7} {'95% CI':>18} {'p_raw':>8} {'p_adj':>8} {'Cliff':>6}")
    print("-" * 90)

    n_confirmed = n_sig_raw_only = 0
    for solver in solvers:
        for density in DENSITY_ORDER:
            for scenario in SCENARIO_ORDER:
                key = (solver, density, scenario)
                rs  = ratios.get(key, [])
                if not rs:
                    continue
                bl = baselines.get(key, [])
                ft = faults.get(key, [])
                r  = mean(rs)
                lo, hi = ci95(rs)
                _, p_raw = mann_whitney_u(ft, bl)
                p_adj = adjusted_p.get(key, float("nan"))
                cd    = cliffs_delta(ft, bl)
                sig_adj = not math.isnan(p_adj) and p_adj < 0.05
                sig_raw = not math.isnan(p_raw) and p_raw < 0.05
                if sig_raw and not sig_adj:
                    n_sig_raw_only += 1

                braess = ""
                if r > 1.0 and lo > 1.0 and sig_adj:
                    braess = " ←CONFIRMED"
                    n_confirmed += 1
                elif r > 1.0 and lo > 1.0:
                    braess = " (CI only)"

                flag   = "**" if sig_adj else ("* " if sig_raw else "  ")
                cd_str = f"{cd:>6.3f}" if not math.isnan(cd) else "   —  "
                print(f"{SOLVER_LABEL.get(solver, solver):<20} {density:>3} {scenario:<25} "
                      f"{r:>7.3f} [{lo:.3f}, {hi:.3f}] {format_p(p_raw):>8} "
                      f"{format_p(p_adj):>8}{flag} {cd_str}{braess}")

    print(f"\n--- {n_confirmed} confirmed Braess (CI>1 + BH-adj. p<0.05)")
    print(f"    {n_sig_raw_only} results lost significance after FDR correction")

    # Braess onset threshold per solver
    print("\n--- Braess onset threshold per solver (permanent-distributed scenarios) ---")
    perm_dist = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high"]
    for solver in solvers:
        threshold = None
        for density in DENSITY_ORDER:
            for sc in perm_dist:
                key = (solver, density, sc)
                rs  = ratios.get(key, [])
                if not rs:
                    continue
                r    = mean(rs)
                lo, _ = ci95(rs)
                p_adj = adjusted_p.get(key, 1.0)
                if r > 1.0 and lo > 1.0 and p_adj < 0.05:
                    threshold = (density, sc)
                    break
            if threshold:
                break
        if threshold:
            print(f"  {SOLVER_LABEL.get(solver, solver):<20} → n={threshold[0]} ({threshold[1]})")
        else:
            print(f"  {SOLVER_LABEL.get(solver, solver):<20} → no confirmed Braess benefit")

    # Token Passing vulnerability check
    if "token_passing" in solvers:
        print("\n--- Token Passing: permanent vs recoverable scenario FT ---")
        perm = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high", "perm_zone_100pct"]
        rec  = ["zone_50t", "intermittent_80m15r"]
        for density in DENSITY_ORDER:
            tp_perm = [r for sc in perm for r in ratios.get(("token_passing", density, sc), [])]
            tp_rec  = [r for sc in rec  for r in ratios.get(("token_passing", density, sc), [])]
            if tp_perm and tp_rec:
                print(f"  n={density:>2}: perm={mean(tp_perm):.3f}  recoverable={mean(tp_rec):.3f}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    print("Loading data...")
    rows = load_runs("braess_resilience_runs.csv", "braess_perm_zone_runs.csv")
    print(f"  {len(rows)} total rows loaded")

    pairs = pair_runs(rows)
    print(f"  {len(pairs)} paired configs")

    ratios, baselines, faults = compute_braess_ratios(pairs)
    solvers = [s for s in SOLVER_ORDER if any(k[0] == s for k in ratios)]
    print(f"  {len(ratios)} (solver, density, scenario) groups — solvers: {solvers}")

    # BH-FDR correction across all tests
    print("\nComputing BH-FDR adjusted p-values...")
    raw_p = []
    for solver in solvers:
        for density in DENSITY_ORDER:
            for scenario in SCENARIO_ORDER:
                key = (solver, density, scenario)
                bl  = baselines.get(key, [])
                ft  = faults.get(key, [])
                if bl and ft:
                    _, p = mann_whitney_u(ft, bl)
                    raw_p.append((key, p))
                else:
                    raw_p.append((key, float("nan")))

    adjusted_p = benjamini_hochberg(raw_p)
    n_raw = sum(1 for _, p in raw_p if not math.isnan(p) and p < 0.05)
    n_adj = sum(1 for p in adjusted_p.values() if not math.isnan(p) and p < 0.05)
    print(f"  Raw p<0.05: {n_raw}  →  BH-adjusted p<0.05: {n_adj}  ({n_raw-n_adj} lost after correction)")

    print("\nWriting outputs...")
    write_ratios_csv(ratios, baselines, faults, adjusted_p)
    write_significance_csv(baselines, faults)
    write_latex_table(ratios, baselines, faults, adjusted_p)
    write_degradation_svg(ratios)

    print_summary(ratios, baselines, faults, adjusted_p)


if __name__ == "__main__":
    main()
