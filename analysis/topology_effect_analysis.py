#!/usr/bin/env python3
"""
Topology Effect Analysis for MAFIS Paper.

RQ2: Does warehouse layout affect fault resilience?

Auto-discovers all topology_*_runs.csv files in results/, computes:
  1. FT ratios per topology × scenario
  2. Mann-Whitney U significance tests (BH-FDR corrected)
  3. LaTeX table (topology × scenario FT matrix)
  4. Heatmap SVG visualization

Usage:
  python3 analysis/topology_effect_analysis.py

Outputs (in results/):
  topology_effect_metrics.csv  -- FT per (topology, scenario)
  topology_effect_table.tex    -- LaTeX table for paper
  topology_effect_heatmap.svg  -- visual heatmap
"""

import csv
import glob
import math
import os
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(__file__))
from stats import (
    load_runs, pair_runs, mean, ci95, mann_whitney_u,
    cliffs_delta, benjamini_hochberg, format_p, RESULTS_DIR,
)
from constants import SCENARIO_ORDER, SCENARIO_LABEL, SCENARIO_LABEL_SHORT


# Preferred display order — covers both old and new topology IDs.
_TOPO_PREF = [
    "warehouse_large", "warehouse_medium",
    "kiva_warehouse",  "kiva_large",
    "sorting_center",
    "compact_grid",
    "fullfilment_center", "fulfillment_center",
]

TOPOLOGY_LABEL = {
    "warehouse_large":    "Warehouse (L)",
    "warehouse_medium":   "Warehouse (M)",
    "kiva_warehouse":     "Kiva Warehouse",
    "kiva_large":         "Kiva (large)",
    "sorting_center":     "Sorting Center",
    "compact_grid":       "Compact Grid",
    "fullfilment_center": "Fulfillment Ctr",
    "fulfillment_center": "Fulfillment Ctr",
}


def discover_topology_csvs():
    """Find all topology_*_runs.csv files in results/."""
    return [
        os.path.basename(p)
        for p in sorted(glob.glob(os.path.join(RESULTS_DIR, "topology_*_runs.csv")))
    ]


def ordered_topologies(found):
    """Sort found topology IDs by _TOPO_PREF, append unknown ones alphabetically."""
    ordered  = [t for t in _TOPO_PREF if t in found]
    leftover = sorted(t for t in found if t not in ordered)
    return ordered + leftover


# ---------------------------------------------------------------------------
# FT ratio computation
# ---------------------------------------------------------------------------

def compute_ft_ratios(pairs):
    """Group by (topology, scenario); compute FT = fault_throughput / baseline."""
    ratios = defaultdict(list)
    baselines = defaultdict(list)
    faults = defaultdict(list)
    for (solver, topology, scenario, scheduler, num_agents, seed), data in pairs.items():
        bl = data.get("baseline", float("nan"))
        ft = data.get("fault", float("nan"))
        if math.isnan(bl) or math.isnan(ft) or bl == 0:
            continue
        key = (topology, scenario)
        ratios[key].append(ft / bl)
        baselines[key].append(bl)
        faults[key].append(ft)
    return ratios, baselines, faults


def ft_color(ratio):
    """Map FT ratio to a hex color. 1.0 = neutral, < 1 = red, > 1 = green."""
    if math.isnan(ratio): return "#2a2a3e"
    if ratio < 0.50:  return "#7b1010"
    if ratio < 0.70:  return "#c0392b"
    if ratio < 0.85:  return "#e67e22"
    if ratio < 0.95:  return "#d4ac0d"
    if ratio < 1.05:  return "#4a5568"
    if ratio < 1.20:  return "#27ae60"
    if ratio < 1.40:  return "#1e8449"
    return "#0d5c34"


# ---------------------------------------------------------------------------
# Output: CSV
# ---------------------------------------------------------------------------

def write_metrics_csv(ratios, baselines, faults, adjusted_p, topos):
    path = os.path.join(RESULTS_DIR, "topology_effect_metrics.csv")
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow([
            "topology", "scenario", "n_seeds",
            "ft_mean", "ft_ci95_lo", "ft_ci95_hi",
            "baseline_mean", "fault_mean",
            "mw_p_raw", "mw_p_adj", "cliffs_d",
        ])
        for topo in topos:
            for scenario in SCENARIO_ORDER:
                key = (topo, scenario)
                rs  = ratios.get(key, [])
                if not rs:
                    continue
                bl = baselines.get(key, [])
                ft = faults.get(key, [])
                r_mean = mean(rs)
                r_lo, r_hi = ci95(rs)
                u, p = mann_whitney_u(ft, bl)
                cd = cliffs_delta(ft, bl)
                p_adj = adjusted_p.get(key, float("nan"))
                w.writerow([
                    topo, scenario, len(rs),
                    f"{r_mean:.4f}", f"{r_lo:.4f}", f"{r_hi:.4f}",
                    f"{mean(bl):.4f}", f"{mean(ft):.4f}",
                    f"{p:.4f}" if not math.isnan(p) else "",
                    f"{p_adj:.4f}" if not math.isnan(p_adj) else "",
                    f"{cd:.3f}" if not math.isnan(cd) else "",
                ])
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Output: LaTeX table
# ---------------------------------------------------------------------------

def write_latex_table(ratios, adjusted_p, topos):
    path = os.path.join(RESULTS_DIR, "topology_effect_table.tex")
    col_spec = "l" + "c" * len(SCENARIO_ORDER)

    lines = [
        "% Topology effect table — auto-generated by analysis/topology_effect_analysis.py",
        "% Cells: mean FT ratio. * BH-adj. p < 0.05.",
        "",
        "\\begin{table}[ht]",
        "  \\caption{FT ratio by topology (RQ2: topology effect). "
        "Controlled: PIBT solver, random scheduler, per-topology agent counts. "
        "$^*$ BH-adj.~$p < 0.05$.}",
        "  \\label{tab:topology_effect}",
        "  \\centering",
        "  \\footnotesize",
        f"  \\begin{{tabular}}{{{col_spec}}}",
        "    \\toprule",
    ]
    sc_hdrs = " & ".join(f"\\textbf{{{SCENARIO_LABEL.get(s, s)}}}" for s in SCENARIO_ORDER)
    lines.append(f"    \\textbf{{Topology}} & {sc_hdrs} \\\\")
    lines.append("    \\midrule")

    for topo in topos:
        cells = [TOPOLOGY_LABEL.get(topo, topo)]
        for scenario in SCENARIO_ORDER:
            key = (topo, scenario)
            rs  = ratios.get(key, [])
            if not rs:
                cells.append("—")
                continue
            r = mean(rs)
            p_adj = adjusted_p.get(key, 1.0)
            sig = not math.isnan(p_adj) and p_adj < 0.05
            cells.append(f"${r:.3f}{'{}^*' if sig else ''}$")
        lines.append("    " + " & ".join(cells) + " \\\\")

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
# Output: Heatmap SVG
# ---------------------------------------------------------------------------

def write_heatmap_svg(ratios, adjusted_p, topos):
    """Topology (rows) × Scenario (cols) heatmap of mean FT ratio."""
    CELL_W, CELL_H = 88, 46
    LEFT   = 132
    TOP    = 82
    RIGHT  = 20
    BOTTOM = 55   # legend row

    W = LEFT + len(SCENARIO_ORDER) * CELL_W + RIGHT
    H = TOP  + len(topos) * CELL_H + BOTTOM

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">',
        f'  <rect width="{W}" height="{H}" fill="#1a1a2e"/>',
        f'  <style>text {{ font-family: monospace; fill: #c8c8d4; }}</style>',
        f'  <text x="{W//2}" y="15" text-anchor="middle" font-size="10" fill="#a0a0c0">'
        f'FT Ratio by Topology × Fault Scenario (1.0 = no change)</text>',
    ]

    # Column headers (rotated)
    for ci, sc in enumerate(SCENARIO_ORDER):
        cx = LEFT + ci * CELL_W + CELL_W // 2
        label = SCENARIO_LABEL_SHORT.get(sc, sc)
        lines.append(
            f'  <text transform="rotate(-40,{cx},{TOP-8})" x="{cx}" y="{TOP-8}" '
            f'text-anchor="end" font-size="10">{label}</text>'
        )

    # Rows
    for ri, topo in enumerate(topos):
        cy_top = TOP + ri * CELL_H
        cy_mid = cy_top + CELL_H // 2
        topo_label = TOPOLOGY_LABEL.get(topo, topo)
        lines.append(
            f'  <text x="{LEFT-6}" y="{cy_mid+4}" text-anchor="end" font-size="11">'
            f'{topo_label}</text>'
        )
        for ci, scenario in enumerate(SCENARIO_ORDER):
            cx  = LEFT + ci * CELL_W
            key = (topo, scenario)
            rs  = ratios.get(key, [])
            r   = mean(rs) if rs else float("nan")
            color = ft_color(r)
            lines.append(
                f'  <rect x="{cx+1}" y="{cy_top+1}" width="{CELL_W-2}" height="{CELL_H-2}" '
                f'fill="{color}" rx="2"/>'
            )
            val  = f"{r:.3f}" if not math.isnan(r) else "—"
            p_adj = adjusted_p.get(key, 1.0)
            sig  = "*" if (not math.isnan(p_adj) and p_adj < 0.05) else ""
            lines.append(
                f'  <text x="{cx+CELL_W//2}" y="{cy_mid+4}" text-anchor="middle" '
                f'font-weight="bold" fill="#fff" font-size="12">{val}{sig}</text>'
            )

    # Legend
    ly = H - BOTTOM + 14
    lx = LEFT
    LEGEND = [
        ("#7b1010", "< 0.50"),
        ("#c0392b", "0.50–0.70"),
        ("#e67e22", "0.70–0.85"),
        ("#d4ac0d", "0.85–0.95"),
        ("#4a5568", "0.95–1.05"),
        ("#27ae60", "1.05–1.20"),
        ("#1e8449", "> 1.20"),
    ]
    for color, label in LEGEND:
        lines.append(f'  <rect x="{lx}" y="{ly}" width="14" height="14" fill="{color}" rx="2"/>')
        lines.append(f'  <text x="{lx+18}" y="{ly+11}" font-size="9">{label}</text>')
        lx += 90

    lines.append("</svg>")

    path = os.path.join(RESULTS_DIR, "topology_effect_heatmap.svg")
    with open(path, "w") as f:
        f.write("\n".join(lines))
    print(f"Saved: {path}")


# ---------------------------------------------------------------------------
# Console summary
# ---------------------------------------------------------------------------

def print_summary(ratios, adjusted_p, topos):
    print("\n" + "=" * 80)
    print("TOPOLOGY EFFECT — RQ2: Does warehouse layout affect fault resilience?")
    print("=" * 80)
    print(f"{'Topology':<22} {'Scenario':<22} {'FT':>7} {'95% CI':>17} {'p_adj':>8}")
    print("-" * 80)

    n_sig = 0
    for topo in topos:
        for scenario in SCENARIO_ORDER:
            key = (topo, scenario)
            rs  = ratios.get(key, [])
            if not rs:
                continue
            r   = mean(rs)
            lo, hi = ci95(rs)
            p_adj  = adjusted_p.get(key, float("nan"))
            sig    = not math.isnan(p_adj) and p_adj < 0.05
            n_sig += sig
            label  = TOPOLOGY_LABEL.get(topo, topo)
            print(f"{label:<22} {scenario:<22} {r:>7.3f} [{lo:.3f},{hi:.3f}] "
                  f"{format_p(p_adj):>9}")
        print()

    print(f"--- {n_sig} significant (BH-adj. p<0.05)")

    print("\n--- Resilience ranking by topology (mean FT across all scenarios) ---")
    ranking = []
    for topo in topos:
        all_rs = [r for sc in SCENARIO_ORDER for r in ratios.get((topo, sc), [])]
        if all_rs:
            ranking.append((mean(all_rs), topo))
    for rank, (m, topo) in enumerate(sorted(ranking, reverse=True), 1):
        label = TOPOLOGY_LABEL.get(topo, topo)
        print(f"  {rank}. {label:<22} mean FT = {m:.3f}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    csvs = discover_topology_csvs()
    if not csvs:
        print(f"No topology_*_runs.csv files found in {RESULTS_DIR}", file=sys.stderr)
        sys.exit(1)

    print(f"Discovered {len(csvs)} topology CSV(s): {', '.join(csvs)}")
    rows = load_runs(*csvs)
    print(f"  {len(rows)} rows loaded")

    pairs = pair_runs(rows)
    print(f"  {len(pairs)} paired configs")

    ratios, baselines, faults = compute_ft_ratios(pairs)
    topos = ordered_topologies(set(k[0] for k in ratios))
    print(f"  Topologies found: {topos}")

    print("\nComputing BH-FDR adjusted p-values...")
    raw_p = []
    for topo in topos:
        for scenario in SCENARIO_ORDER:
            key = (topo, scenario)
            bl  = baselines.get(key, [])
            ft  = faults.get(key, [])
            if bl and ft:
                _, p = mann_whitney_u(ft, bl)
                raw_p.append((key, p))
            else:
                raw_p.append((key, float("nan")))

    adjusted_p = benjamini_hochberg(raw_p)
    n_adj = sum(1 for p in adjusted_p.values() if not math.isnan(p) and p < 0.05)
    print(f"  BH-adjusted significant: {n_adj} / {len(raw_p)}")

    print("\nWriting outputs...")
    write_metrics_csv(ratios, baselines, faults, adjusted_p, topos)
    write_latex_table(ratios, adjusted_p, topos)
    write_heatmap_svg(ratios, adjusted_p, topos)

    print_summary(ratios, adjusted_p, topos)


if __name__ == "__main__":
    main()
