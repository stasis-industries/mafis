#!/usr/bin/env python3
"""
Scheduler Effect Analysis for MAFIS Paper.

RQ4: Does task assignment strategy affect fault resilience?

Reads results/scheduler_effect_runs.csv, computes:
  1. FT ratios per scheduler × scenario
  2. Pairwise random vs closest comparison (Mann-Whitney, BH-FDR)
  3. LaTeX table (scheduler × scenario FT matrix with Cliff's delta row)
  4. Grouped bar chart SVG

Usage:
  python3 analysis/scheduler_effect_analysis.py

Outputs (in results/):
  scheduler_effect_metrics.csv  -- FT per (scheduler, scenario)
  scheduler_effect_table.tex    -- LaTeX table for paper
  scheduler_effect_contrast.svg -- grouped bar chart
"""

import csv
import math
import os
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(__file__))
from stats import (
    load_runs, pair_runs, mean, ci95, mann_whitney_u,
    cliffs_delta, benjamini_hochberg, format_p, RESULTS_DIR,
)
from constants import (
    SCENARIO_ORDER, SCENARIO_LABEL, SCENARIO_LABEL_SHORT,
    SCHEDULER_ORDER, SCHEDULER_LABEL, SCHEDULER_COLORS,
)


# ---------------------------------------------------------------------------
# FT ratio computation
# ---------------------------------------------------------------------------

def compute_ft_ratios(pairs):
    """Group by (scheduler, scenario); compute FT = fault_throughput / baseline."""
    ratios = defaultdict(list)
    baselines = defaultdict(list)
    faults = defaultdict(list)
    for (solver, topology, scenario, scheduler, num_agents, seed), data in pairs.items():
        bl = data.get("baseline", float("nan"))
        ft = data.get("fault", float("nan"))
        if math.isnan(bl) or math.isnan(ft) or bl == 0:
            continue
        key = (scheduler, scenario)
        ratios[key].append(ft / bl)
        baselines[key].append(bl)
        faults[key].append(ft)
    return ratios, baselines, faults


# ---------------------------------------------------------------------------
# Pairwise scheduler comparison
# ---------------------------------------------------------------------------

def compare_schedulers(ratios):
    """
    For each scenario: test H0 that random and closest have equal FT distributions.
    Returns list of (scenario, p_raw, p_adj, cliffs_d, direction).
    direction: 'closest>' | 'random>' | '≈'
    """
    raw_p = []
    for scenario in SCENARIO_ORDER:
        a = ratios.get(("random",  scenario), [])
        b = ratios.get(("closest", scenario), [])
        if a and b:
            _, p = mann_whitney_u(a, b)
            raw_p.append((scenario, p))
        else:
            raw_p.append((scenario, float("nan")))

    adj = benjamini_hochberg(raw_p)

    results = []
    for scenario, p_raw in raw_p:
        a  = ratios.get(("random",  scenario), [])
        b  = ratios.get(("closest", scenario), [])
        cd = cliffs_delta(b, a)          # positive = closest > random
        ra, rb = mean(a), mean(b)
        direction = "closest>" if rb > ra + 0.01 else ("random>" if ra > rb + 0.01 else "≈")
        results.append((scenario, p_raw, adj.get(scenario, float("nan")), cd, direction))
    return results


# ---------------------------------------------------------------------------
# Output: CSV
# ---------------------------------------------------------------------------

def write_metrics_csv(ratios, baselines, faults, comparison):
    path = os.path.join(RESULTS_DIR, "scheduler_effect_metrics.csv")
    comp = {sc: (p_raw, p_adj, cd) for sc, p_raw, p_adj, cd, _ in comparison}
    schedulers = [s for s in SCHEDULER_ORDER if any(k[0] == s for k in ratios)]

    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow([
            "scheduler", "scenario", "n_seeds",
            "ft_mean", "ft_ci95_lo", "ft_ci95_hi",
            "comparison_p_raw", "comparison_p_adj", "cliffs_d",
        ])
        for sched in schedulers:
            for scenario in SCENARIO_ORDER:
                key = (sched, scenario)
                rs  = ratios.get(key, [])
                if not rs:
                    continue
                r_mean = mean(rs)
                r_lo, r_hi = ci95(rs)
                p_raw, p_adj, cd = comp.get(scenario, (float("nan"),) * 3)
                w.writerow([
                    sched, scenario, len(rs),
                    f"{r_mean:.4f}", f"{r_lo:.4f}", f"{r_hi:.4f}",
                    f"{p_raw:.4f}" if not math.isnan(p_raw) else "",
                    f"{p_adj:.4f}" if not math.isnan(p_adj) else "",
                    f"{cd:.3f}"   if not math.isnan(cd)    else "",
                ])
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: LaTeX table
# ---------------------------------------------------------------------------

def write_latex_table(ratios, comparison):
    path = os.path.join(RESULTS_DIR, "scheduler_effect_table.tex")
    comp = {sc: (p_adj, cd) for sc, _, p_adj, cd, _ in comparison}
    schedulers = [s for s in SCHEDULER_ORDER if any(k[0] == s for k in ratios)]
    col_spec = "l" + "c" * len(SCENARIO_ORDER)

    lines = [
        "% Scheduler effect table — auto-generated by analysis/scheduler_effect_analysis.py",
        "% Cells: mean FT ratio. * BH-adj. p < 0.05 for random vs closest comparison.",
        "",
        "\\begin{table}[ht]",
        "  \\caption{FT ratio by scheduler (RQ4: scheduler effect). "
        "Controlled: PIBT solver, warehouse topology, 40 agents. "
        "Cliff's $d > 0$ = Closest achieves higher FT.}",
        "  \\label{tab:scheduler_effect}",
        "  \\centering",
        "  \\footnotesize",
        f"  \\begin{{tabular}}{{{col_spec}}}",
        "    \\toprule",
    ]
    sc_hdrs = " & ".join(f"\\textbf{{{SCENARIO_LABEL.get(s, s)}}}" for s in SCENARIO_ORDER)
    lines.append(f"    \\textbf{{Scheduler}} & {sc_hdrs} \\\\")
    lines.append("    \\midrule")

    for sched in schedulers:
        cells = [SCHEDULER_LABEL.get(sched, sched)]
        for scenario in SCENARIO_ORDER:
            key = (sched, scenario)
            rs  = ratios.get(key, [])
            if not rs:
                cells.append("—")
                continue
            r = mean(rs)
            p_adj, _ = comp.get(scenario, (1.0, float("nan")))
            sig = not math.isnan(p_adj) and p_adj < 0.05
            cells.append(f"${r:.3f}{'{}^*' if sig else ''}$")
        lines.append("    " + " & ".join(cells) + " \\\\")

    # Cliff's delta row
    lines.append("    \\midrule")
    cd_cells = ["\\textit{Cliff's~$d$}"]
    for scenario in SCENARIO_ORDER:
        _, cd = comp.get(scenario, (1.0, float("nan")))
        cd_cells.append(f"${cd:.3f}$" if not math.isnan(cd) else "—")
    lines.append("    " + " & ".join(cd_cells) + " \\\\")

    lines += [
        "    \\bottomrule",
        f"    \\multicolumn{{{len(SCENARIO_ORDER)+1}}}{{l}}{{",
        "      \\footnotesize $^*$ BH-adj.~$p < 0.05$.}}",
        "  \\end{tabular}",
        "\\end{table}",
    ]

    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: SVG contrast bar chart
# ---------------------------------------------------------------------------

def write_contrast_svg(ratios):
    """Grouped bars: random vs closest, one group per scenario."""
    schedulers = [s for s in SCHEDULER_ORDER if any(k[0] == s for k in ratios)]
    scenarios  = [s for s in SCENARIO_ORDER if any((sc, s) in ratios for sc in schedulers)]

    BAR_W     = 26
    GROUP_GAP = 20
    GROUP_W   = len(schedulers) * BAR_W + GROUP_GAP

    LEFT, TOP, RIGHT, BOTTOM = 55, 40, 20, 90
    panel_w = len(scenarios) * GROUP_W
    W = LEFT + panel_w + RIGHT

    all_means = [mean(ratios.get((s, sc), [])) for s in schedulers for sc in scenarios
                 if ratios.get((s, sc))]
    y_max = max((m for m in all_means if not math.isnan(m)), default=1.2) + 0.10
    y_min = max(0.0, min((m for m in all_means if not math.isnan(m)), default=0.8) - 0.10)
    y_min = min(y_min, 0.85)
    y_max = max(y_max, 1.10)

    ph    = 220
    H     = TOP + ph + BOTTOM
    y_range = y_max - y_min
    y_scale = ph / y_range if y_range else ph

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">',
        f'  <rect width="{W}" height="{H}" fill="#1a1a2e"/>',
        f'  <style>text {{ font-family: monospace; fill: #c8c8d4; }}</style>',
        f'  <rect x="{LEFT}" y="{TOP}" width="{panel_w}" height="{ph}" fill="#12122a" rx="2"/>',
        f'  <text x="{LEFT+panel_w//2}" y="22" text-anchor="middle" font-size="11" fill="#a0a0c0">'
        f'Scheduler Effect: FT Ratio per Fault Scenario</text>',
    ]

    # Grid + y labels
    for tick in [y for y in [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.2] if y_min <= y <= y_max]:
        ty = TOP + ph - (tick - y_min) * y_scale
        style = 'stroke="#556" stroke-width="1.5" stroke-dasharray="5,3"' \
                if abs(tick - 1.0) < 0.001 else 'stroke="#333" stroke-width="1" stroke-dasharray="3,3"'
        lines.append(f'  <line x1="{LEFT}" y1="{ty:.1f}" x2="{LEFT+panel_w}" y2="{ty:.1f}" {style}/>')
        lines.append(f'  <text x="{LEFT-4}" y="{ty+4:.1f}" text-anchor="end" font-size="9">{tick:.2f}</text>')

    # Bars
    for gi, scenario in enumerate(scenarios):
        gx = LEFT + gi * GROUP_W + GROUP_GAP // 2
        for bi, sched in enumerate(schedulers):
            rs = ratios.get((sched, scenario), [])
            if not rs:
                continue
            r  = mean(rs)
            lo, hi = ci95(rs)
            color  = SCHEDULER_COLORS.get(sched, "#888")

            bx     = gx + bi * BAR_W + 2
            bar_h  = (r - y_min) * y_scale
            bar_y  = TOP + ph - bar_h

            lines.append(
                f'  <rect x="{bx}" y="{bar_y:.1f}" width="{BAR_W-4}" '
                f'height="{bar_h:.1f}" fill="{color}" opacity="0.85" rx="1"/>'
            )
            # Error bar
            cx_b   = bx + (BAR_W - 4) // 2
            err_lo = TOP + ph - (lo - y_min) * y_scale
            err_hi = TOP + ph - (hi - y_min) * y_scale
            lines.append(
                f'  <line x1="{cx_b}" y1="{err_hi:.1f}" x2="{cx_b}" y2="{err_lo:.1f}" '
                f'stroke="#fff" stroke-width="1.5"/>'
            )
            lines.append(
                f'  <line x1="{cx_b-3}" y1="{err_hi:.1f}" x2="{cx_b+3}" y2="{err_hi:.1f}" '
                f'stroke="#fff" stroke-width="1.5"/>'
            )

        # Scenario label (rotated)
        sc_cx = gx + len(schedulers) * BAR_W // 2
        label = SCENARIO_LABEL_SHORT.get(scenario, scenario)
        lines.append(
            f'  <text transform="rotate(-35,{sc_cx},{TOP+ph+14})" '
            f'x="{sc_cx}" y="{TOP+ph+14}" text-anchor="end" font-size="9">{label}</text>'
        )

    # Axis lines
    lines.append(f'  <line x1="{LEFT}" y1="{TOP}" x2="{LEFT}" y2="{TOP+ph}" stroke="#444" stroke-width="1"/>')
    lines.append(f'  <line x1="{LEFT}" y1="{TOP+ph}" x2="{LEFT+panel_w}" y2="{TOP+ph}" stroke="#444" stroke-width="1"/>')

    # Legend
    lx, ly = LEFT, H - 18
    for sched in schedulers:
        color = SCHEDULER_COLORS.get(sched, "#888")
        label = SCHEDULER_LABEL.get(sched, sched)
        lines.append(f'  <rect x="{lx}" y="{ly-10}" width="14" height="14" fill="{color}" rx="1"/>')
        lines.append(f'  <text x="{lx+18}" y="{ly}" font-size="9">{label}</text>')
        lx += 80

    # y-axis title
    lines.append(
        f'  <text transform="rotate(-90)" x="{-(TOP+ph//2)}" y="14" '
        f'text-anchor="middle" font-size="10">FT Ratio (fault / baseline)</text>'
    )

    lines.append("</svg>")

    path = os.path.join(RESULTS_DIR, "scheduler_effect_contrast.svg")
    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Console summary
# ---------------------------------------------------------------------------

def print_summary(ratios, comparison):
    schedulers = [s for s in SCHEDULER_ORDER if any(k[0] == s for k in ratios)]

    print("\n" + "=" * 82)
    print("SCHEDULER EFFECT — RQ4: Does scheduler affect fault resilience?")
    print("=" * 82)

    header = f"{'Scenario':<22}"
    for s in schedulers:
        header += f"  {SCHEDULER_LABEL.get(s, s):>10}"
    header += f"  {'p_adj':>8}  {'Cliff d':>7}  winner"
    print(header)
    print("-" * 82)

    n_sig = n_closest_better = 0
    for scenario, p_raw, p_adj, cd, direction in comparison:
        row = f"{scenario:<22}"
        for sched in schedulers:
            rs = ratios.get((sched, scenario), [])
            r  = mean(rs) if rs else float("nan")
            row += f"  {r:>10.3f}" if not math.isnan(r) else f"  {'—':>10}"
        sig = not math.isnan(p_adj) and p_adj < 0.05
        n_sig          += sig
        n_closest_better += (direction == "closest>")
        cd_str  = f"{cd:.3f}" if not math.isnan(cd) else "—"
        row += f"  {format_p(p_adj):>8}  {cd_str:>7}  {direction}"
        if sig:
            row += "  *"
        print(row)

    print(f"\n--- {n_sig}/{len(comparison)} scenarios: scheduler difference significant (BH-adj. p<0.05)")
    print(f"--- Closest has higher mean FT in {n_closest_better}/{len(comparison)} scenarios")

    print("\n--- Overall mean FT per scheduler ---")
    for sched in schedulers:
        all_rs = [r for sc in SCENARIO_ORDER for r in ratios.get((sched, sc), [])]
        if all_rs:
            print(f"  {SCHEDULER_LABEL.get(sched, sched):<12}: mean FT = {mean(all_rs):.3f}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    print("Loading scheduler_effect_runs.csv...")
    rows = load_runs("scheduler_effect_runs.csv")
    print(f"  {len(rows)} rows loaded")

    pairs = pair_runs(rows)
    print(f"  {len(pairs)} paired configs")

    ratios, baselines, faults = compute_ft_ratios(pairs)
    schedulers = sorted(set(k[0] for k in ratios))
    print(f"  {len(ratios)} groups — schedulers: {schedulers}")

    print("\nComparing schedulers per scenario (BH-FDR)...")
    comparison = compare_schedulers(ratios)
    n_sig = sum(1 for _, _, p_adj, _, _ in comparison if not math.isnan(p_adj) and p_adj < 0.05)
    print(f"  {n_sig}/{len(comparison)} scenarios: significant difference")

    print("\nWriting outputs...")
    write_metrics_csv(ratios, baselines, faults, comparison)
    write_latex_table(ratios, comparison)
    write_contrast_svg(ratios)

    print_summary(ratios, comparison)


if __name__ == "__main__":
    main()
