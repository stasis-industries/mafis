"""
PAAMS 2026 — Full analysis script.
Produces: tables (CSV + LaTeX), figures (SVG/PDF), and a summary text report.

Run:  python3 results/analyze_paams.py
Outputs land in results/figures/ and results/tables/
"""

import os, textwrap
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.colors as mcolors
from matplotlib.patches import Patch

# ── paths ────────────────────────────────────────────────────────────────────
RESULTS = os.path.dirname(os.path.abspath(__file__))
FIG_DIR  = os.path.join(RESULTS, "figures")
TAB_DIR  = os.path.join(RESULTS, "tables")
os.makedirs(FIG_DIR, exist_ok=True)
os.makedirs(TAB_DIR, exist_ok=True)

# ── load ─────────────────────────────────────────────────────────────────────
wl   = pd.read_csv(os.path.join(RESULTS, "warehouse_large_summary.csv"))
kiva = pd.read_csv(os.path.join(RESULTS, "kiva_warehouse_summary.csv"))
cg   = pd.read_csv(os.path.join(RESULTS, "compact_grid_summary.csv"))
sched = pd.read_csv(os.path.join(RESULTS, "scheduler_effect_summary.csv"))

all_e1 = pd.concat([wl, kiva, cg], ignore_index=True)

# ── display names ─────────────────────────────────────────────────────────────
SOLVER_LABELS = {
    "pibt":            "PIBT",
    "rhcr_pbs":        "RHCR-PBS",
    "token_passing":   "Token-P",
    "lacam3_lifelong": "LaCAM3",
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
    "warehouse_large": "Warehouse-L\n(57×33, 40ag)",
    "kiva_warehouse":  "KIVA\n(48×48, 80ag)",
    "compact_grid":    "Compact\n(26×26, 25ag)",
}
SCENARIO_ORDER = list(SCENARIO_LABELS.keys())
SOLVER_ORDER   = list(SOLVER_LABELS.keys())

FAULT_CATS = {
    "burst_20pct":           "Permanent",
    "burst_50pct":           "Permanent",
    "wear_medium":           "Permanent",
    "wear_high":             "Permanent",
    "zone_50t":              "Recoverable",
    "intermittent_80s80m15r":"Recoverable",
}

# ── colour palette ────────────────────────────────────────────────────────────
SOLVER_COLORS = {
    "pibt":            "#2196F3",
    "rhcr_pbs":        "#9C27B0",
    "token_passing":   "#4CAF50",
    "lacam3_lifelong": "#F44336",
}

# ─────────────────────────────────────────────────────────────────────────────
# Helper: 95% CI label  ±X.XXX
# ─────────────────────────────────────────────────────────────────────────────
def ci_label(mean, lo, hi):
    if pd.isna(mean): return "N/A"
    half = (hi - lo) / 2
    return f"{mean:.3f} ±{half:.3f}"

# =============================================================================
# TABLE 1 — Solver × Fault Scenario (FT + Critical-Time + Cascade Spread)
#            E1 default density per topology, averaged across topologies
# =============================================================================
print("Building Table 1: Solver × Scenario …")

# Use the middle density for each topology: wl=40, kiva=80, cg=25
DEFAULT_DENSITY = {"warehouse_large": 40, "kiva_warehouse": 80, "compact_grid": 25}

t1_rows = []
for topo, n in DEFAULT_DENSITY.items():
    src = all_e1[(all_e1.topology == topo) & (all_e1.num_agents == n)]
    t1_rows.append(src)
t1 = pd.concat(t1_rows, ignore_index=True)

# Average across topologies for the cross-topology summary table
t1_agg = (
    t1.groupby(["solver", "scenario"])
      .agg(
          ft_mean=("ft_mean","mean"),
          ft_lo=("ft_ci95_lo","mean"),
          ft_hi=("ft_ci95_hi","mean"),
          ct_mean=("critical_time_mean","mean"),
          tp_mean=("throughput_mean","mean"),
          cascade_spread_mean=("cascade_spread_mean","mean"),
          fleet_util_mean=("fleet_utilization_mean","mean"),
          n=("num_seeds","mean"),
      )
      .reset_index()
)

# Wide pivot: solvers as rows, scenarios as columns, value = FT ±CI
ft_pivot = t1_agg.pivot(index="solver", columns="scenario", values="ft_mean")
ft_pivot = ft_pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)

ct_pivot = t1_agg.pivot(index="solver", columns="scenario", values="ct_mean")
ct_pivot = ct_pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)

cascade_spread_pivot = t1_agg.pivot(index="solver", columns="scenario", values="cascade_spread_mean")
cascade_spread_pivot = cascade_spread_pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)
fleet_util_pivot = t1_agg.pivot(index="solver", columns="scenario", values="fleet_util_mean")
fleet_util_pivot = fleet_util_pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)

# Save CSV
ft_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).to_csv(
    os.path.join(TAB_DIR, "table1_ft.csv")
)
ct_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).to_csv(
    os.path.join(TAB_DIR, "table1_critical_time.csv")
)

# LaTeX
def df_to_latex(df, caption, label, fmt=".3f"):
    col_fmt = "l" + "r"*len(df.columns)
    lines = [
        r"\begin{table}[htbp]",
        r"  \centering",
        r"  \small",
        f"  \\caption{{{caption}}}",
        f"  \\label{{{label}}}",
        f"  \\begin{{tabular}}{{{col_fmt}}}",
        r"    \toprule",
        "    Solver & " + " & ".join(df.columns) + r" \\",
        r"    \midrule",
    ]
    for row_idx, row in df.iterrows():
        vals = []
        for v in row:
            if pd.isna(v):
                vals.append("--")
            elif abs(v) > 1.5:
                vals.append(f"\\textbf{{{v:{fmt}}}}")  # bold Braess cases
            else:
                vals.append(f"{v:{fmt}}")
        lines.append(f"    {row_idx} & " + " & ".join(vals) + r" \\")
    lines += [r"    \bottomrule", r"  \end{tabular}", r"\end{table}"]
    return "\n".join(lines)

with open(os.path.join(TAB_DIR, "table1_ft.tex"), "w") as f:
    f.write(df_to_latex(
        ft_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS),
        "Fault Tolerance (FT = P\\textsubscript{fault}/P\\textsubscript{baseline}) by Solver and Fault Scenario. "
        "Values $>$1.0 indicate Braess paradox. Bold = FT$>$1.5.",
        "tab:solver_ft"
    ))

with open(os.path.join(TAB_DIR, "table1_critical_time.tex"), "w") as f:
    f.write(df_to_latex(
        ct_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS),
        "Critical Time (fraction of ticks below 50\\% of baseline throughput). Lower is better.",
        "tab:critical_time"
    ))

print(f"  → tables/table1_ft.csv, table1_ft.tex, table1_critical_time.*")

# =============================================================================
# TABLE 2 — Topology Comparison: FT + Throughput by topology (PIBT, 3 densities)
# =============================================================================
print("Building Table 2: Topology × Density × Scenario …")

pibt_e1 = all_e1[all_e1.solver == "pibt"].copy()
pibt_e1["topo_label"] = pibt_e1.topology.map(lambda t: TOPO_LABELS.get(t, t))

# Pivot: FT by (topology, num_agents) vs scenario
topo_ft = (
    pibt_e1.groupby(["topology","num_agents","scenario"])["ft_mean"]
    .mean()
    .reset_index()
)
topo_ft["config"] = topo_ft.apply(
    lambda r: f"{TOPO_LABELS.get(r.topology, r.topology).split(chr(10))[0]} n={int(r.num_agents)}", axis=1
)
topo_pivot = topo_ft.pivot(index="config", columns="scenario", values="ft_mean")
topo_pivot = topo_pivot.reindex(columns=SCENARIO_ORDER)
topo_pivot.rename(columns=SCENARIO_LABELS).to_csv(os.path.join(TAB_DIR, "table2_topology_ft.csv"))

with open(os.path.join(TAB_DIR, "table2_topology_ft.tex"), "w") as f:
    f.write(df_to_latex(
        topo_pivot.rename(columns=SCENARIO_LABELS),
        "PIBT Fault Tolerance across topology × density configurations. "
        "Bold = FT$>$1.5.",
        "tab:topology_ft"
    ))
print(f"  → tables/table2_topology_ft.*")

# =============================================================================
# TABLE 3 — Scheduler Effect: closest vs random (warehouse_large, 40 agents)
# =============================================================================
print("Building Table 3: Scheduler Effect …")

sched_sub = sched[(sched.topology == "warehouse_large") & (sched.num_agents == 40)].copy()
sched_sub["sched_solver"] = sched_sub.scheduler + " / " + sched_sub.solver.map(SOLVER_LABELS)

sched_ft = sched_sub.pivot_table(
    index="solver", columns=["scheduler","scenario"], values="ft_mean"
)

# Simpler: per-scenario delta (closest - random)
sched_delta = []
for solver in SOLVER_ORDER:
    for scen in SCENARIO_ORDER:
        r_closest = sched_sub[(sched_sub.solver==solver)&(sched_sub.scheduler=="closest")&(sched_sub.scenario==scen)]
        r_random  = sched_sub[(sched_sub.solver==solver)&(sched_sub.scheduler=="random") &(sched_sub.scenario==scen)]
        if r_closest.empty or r_random.empty: continue
        ft_c = r_closest.ft_mean.values[0]
        ft_r = r_random.ft_mean.values[0]
        tp_c = r_closest.throughput_mean.values[0]
        tp_r = r_random.throughput_mean.values[0]
        idle_c = r_closest.get("idle_ratio_mean", pd.Series([np.nan])).values[0] if "idle_ratio_mean" in r_closest.columns else np.nan
        idle_r = r_random.get("idle_ratio_mean", pd.Series([np.nan])).values[0]  if "idle_ratio_mean" in r_random.columns  else np.nan
        sched_delta.append({
            "solver": solver,
            "scenario": scen,
            "ft_closest": ft_c,
            "ft_random":  ft_r,
            "ft_delta":   ft_c - ft_r if not (pd.isna(ft_c) or pd.isna(ft_r)) else np.nan,
            "tp_closest": tp_c,
            "tp_random":  tp_r,
            "tp_delta_pct": (tp_c - tp_r) / tp_r * 100 if tp_r and tp_r > 0 else np.nan,
        })
sched_df = pd.DataFrame(sched_delta)

# Summary: mean TP delta (closest - random) per solver
sched_tp_summary = (
    sched_df.groupby("solver")["tp_delta_pct"].mean()
    .rename(SOLVER_LABELS)
    .sort_values(ascending=False)
)
sched_ft_summary = (
    sched_df.groupby("solver")["ft_delta"].mean()
    .rename(SOLVER_LABELS)
    .sort_values(ascending=False)
)

pd.DataFrame({"tp_delta_pct": sched_tp_summary, "ft_delta": sched_ft_summary}).to_csv(
    os.path.join(TAB_DIR, "table3_scheduler_summary.csv")
)

# Full pivot: solver × scenario, value = TP delta %
sched_tp_pivot = sched_df.pivot(index="solver", columns="scenario", values="tp_delta_pct")
sched_tp_pivot = sched_tp_pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)
sched_tp_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).to_csv(
    os.path.join(TAB_DIR, "table3_scheduler_tp_delta.csv")
)
print(f"  → tables/table3_scheduler_*")

# =============================================================================
# TABLE 4 — Braess Paradox Inventory
# =============================================================================
print("Building Table 4: Braess Paradox Cases …")

braess = all_e1[all_e1.ft_mean > 1.0].copy()
braess["solver_label"] = braess.solver.map(SOLVER_LABELS)
braess["scenario_label"] = braess.scenario.map(SCENARIO_LABELS)
braess["topo_short"] = braess.topology.map({
    "warehouse_large": "Wh-L",
    "kiva_warehouse":  "KIVA",
    "compact_grid":    "CG",
})
braess["category"] = braess.scenario.map(FAULT_CATS)

braess_table = braess[[
    "solver_label","topo_short","num_agents","scenario_label","category",
    "ft_mean","ft_ci95_lo","ft_ci95_hi","throughput_mean","survival_rate_mean","num_seeds"
]].sort_values("ft_mean", ascending=False).rename(columns={
    "solver_label":"Solver","topo_short":"Topo","num_agents":"N",
    "scenario_label":"Scenario","category":"Category",
    "ft_mean":"FT","ft_ci95_lo":"CI_lo","ft_ci95_hi":"CI_hi",
    "throughput_mean":"TP","survival_rate_mean":"Survival","num_seeds":"Seeds",
})
braess_table.to_csv(os.path.join(TAB_DIR, "table4_braess_cases.csv"), index=False)

print(f"  → {len(braess_table)} Braess cases found → tables/table4_braess_cases.csv")

# LaTeX summary
with open(os.path.join(TAB_DIR, "table4_braess_cases.tex"), "w") as f:
    f.write(r"\begin{table}[htbp]" + "\n")
    f.write(r"  \centering\small" + "\n")
    f.write(r"  \caption{Braess Paradox instances (FT $>$ 1.0): configurations where fault-induced agent removal \emph{improves} system throughput. Sorted by FT descending.}" + "\n")
    f.write(r"  \label{tab:braess}" + "\n")
    f.write(r"  \begin{tabular}{llrllrrr}" + "\n")
    f.write(r"    \toprule" + "\n")
    f.write(r"    Solver & Topo & N & Scenario & Category & FT & 95\% CI & TP \\" + "\n")
    f.write(r"    \midrule" + "\n")
    for _, row in braess_table.iterrows():
        ci_str = f"[{row['CI_lo']:.2f}, {row['CI_hi']:.2f}]"
        ft_str = f"\\textbf{{{row['FT']:.3f}}}" if row["FT"] > 1.5 else f"{row['FT']:.3f}"
        f.write(f"    {row['Solver']} & {row['Topo']} & {int(row['N'])} & {row['Scenario']} & {row['Category']} & {ft_str} & {ci_str} & {row['TP']:.3f} \\\\\n")
    f.write(r"    \bottomrule" + "\n")
    f.write(r"  \end{tabular}" + "\n")
    f.write(r"\end{table}" + "\n")
print(f"  → tables/table4_braess_cases.tex")

# =============================================================================
# FIGURE 1 — FT Heatmap: Solver × Scenario, averaged across topologies
# =============================================================================
print("Building Figure 1: FT Heatmap …")

fig, axes = plt.subplots(1, 3, figsize=(18, 5.5), sharey=True)
fig.suptitle("Fault Tolerance (FT = P_fault / P_baseline) — PAAMS 2026",
             fontsize=13, fontweight="bold")

topo_info = [
    ("warehouse_large", 40,  "Warehouse-L  (57×33, 40 agents)"),
    ("kiva_warehouse",  80,  "KIVA Warehouse  (48×48, 80 agents)"),
    ("compact_grid",    25,  "Compact Grid  (26×26, 25 agents)"),
]

for ax, (topo, n, title) in zip(axes, topo_info):
    sub = all_e1[(all_e1.topology == topo) & (all_e1.num_agents == n)]
    pivot = sub.pivot_table(index="solver", columns="scenario", values="ft_mean")
    pivot = pivot.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)

    # Custom diverging colormap: white at 1.0
    cmap = plt.cm.RdYlGn
    vmin, vmax = 0.0, 2.0
    norm = mcolors.TwoSlopeNorm(vmin=vmin, vcenter=1.0, vmax=vmax)

    im = ax.imshow(pivot.values, cmap=cmap, norm=norm, aspect="auto")

    # Annotate cells
    for i in range(len(SOLVER_ORDER)):
        for j in range(len(SCENARIO_ORDER)):
            v = pivot.values[i, j]
            if np.isnan(v):
                ax.text(j, i, "N/A", ha="center", va="center", fontsize=8, color="#888")
            else:
                weight = "bold" if v > 1.5 else "normal"
                color  = "white" if v < 0.35 or v > 1.7 else "black"
                ax.text(j, i, f"{v:.2f}", ha="center", va="center",
                        fontsize=8.5, fontweight=weight, color=color)

    ax.set_title(title, fontsize=10, pad=6)
    ax.set_xticks(range(len(SCENARIO_ORDER)))
    ax.set_xticklabels([SCENARIO_LABELS[s] for s in SCENARIO_ORDER],
                       rotation=35, ha="right", fontsize=8)
    ax.set_yticks(range(len(SOLVER_ORDER)))
    if ax == axes[0]:
        ax.set_yticklabels([SOLVER_LABELS[s] for s in SOLVER_ORDER], fontsize=9)
    else:
        ax.set_yticklabels([])

    # Fault-category divider: after col 3 (Permanent | Recoverable)
    # Order: burst_20, burst_50, wear_medium, wear_high | zone_50t, intermittent
    ax.axvline(3.5, color="#333", linewidth=1.2, linestyle="--")

plt.colorbar(im, ax=axes[-1], label="FT (1.0 = baseline)", shrink=0.8)
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig1_ft_heatmap.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig1_ft_heatmap.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig1_ft_heatmap.pdf/.svg")

# =============================================================================
# FIGURE 2 — Throughput vs Density (PIBT, all scenarios, all topologies)
# =============================================================================
print("Building Figure 2: Throughput × Density curves …")

fig, axes = plt.subplots(1, 3, figsize=(16, 4.5), sharey=False)
fig.suptitle("PIBT Throughput vs Fleet Density — by Fault Scenario", fontsize=12, fontweight="bold")

SCEN_COLORS = {
    "burst_20pct":           "#2196F3",
    "burst_50pct":           "#0D47A1",
    "wear_medium":           "#FF9800",
    "wear_high":             "#E65100",
    "zone_50t":              "#4CAF50",
    "intermittent_80s80m15r":"#00796B",
}

for ax, (topo, topo_title, densities) in zip(axes, [
    ("warehouse_large", "Warehouse-L (57×33)", [20, 40, 60]),
    ("kiva_warehouse",  "KIVA (48×48)",        [40, 80, 120]),
    ("compact_grid",    "Compact (26×26)",      [12, 25, 40]),
]):
    sub = pibt_e1[pibt_e1.topology == topo].copy()
    for scen in SCENARIO_ORDER:
        rows = sub[sub.scenario == scen].sort_values("num_agents")
        if rows.empty: continue
        ax.plot(rows.num_agents, rows.throughput_mean,
                marker="o", linewidth=2, markersize=5,
                color=SCEN_COLORS[scen], label=SCENARIO_LABELS[scen])
        # CI shading
        ax.fill_between(rows.num_agents,
                        rows.throughput_ci95_lo, rows.throughput_ci95_hi,
                        alpha=0.12, color=SCEN_COLORS[scen])

    ax.set_title(topo_title, fontsize=10)
    ax.set_xlabel("Fleet size (agents)", fontsize=9)
    ax.set_ylabel("Mean throughput (tasks/tick)", fontsize=9)
    ax.set_xticks(densities)
    ax.grid(True, alpha=0.3)

axes[-1].legend(fontsize=7.5, loc="upper left", framealpha=0.9)
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig2_throughput_density.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig2_throughput_density.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig2_throughput_density.pdf/.svg")

# =============================================================================
# FIGURE 3 — Solver Comparison Radar: FT across all 7 scenarios (warehouse_large, n=40)
# =============================================================================
print("Building Figure 3: Radar charts …")

wl40 = all_e1[(all_e1.topology == "warehouse_large") & (all_e1.num_agents == 40)].copy()

categories = SCENARIO_ORDER
N = len(categories)
angles = np.linspace(0, 2*np.pi, N, endpoint=False).tolist()
angles += angles[:1]  # close

fig, ax = plt.subplots(1, 1, figsize=(8, 7), subplot_kw=dict(polar=True))
ax.set_title("Fault Tolerance by Scenario\n(Warehouse-L, n=40 agents, 6 scenarios)", fontsize=11, pad=20)

for solver in SOLVER_ORDER:
    rows = wl40[wl40.solver == solver]
    vals = []
    for s in categories:
        r = rows[rows.scenario == s]
        vals.append(r.ft_mean.values[0] if not r.empty and not pd.isna(r.ft_mean.values[0]) else 0.0)
    vals_closed = vals + vals[:1]
    ax.plot(angles, vals_closed, linewidth=2,
            color=SOLVER_COLORS[solver], label=SOLVER_LABELS[solver])
    ax.fill(angles, vals_closed, alpha=0.08, color=SOLVER_COLORS[solver])

ax.set_xticks(angles[:-1])
ax.set_xticklabels([SCENARIO_LABELS[s] for s in categories], fontsize=9)
ax.set_ylim(0, 2.5)
ax.set_yticks([0.25, 0.5, 0.75, 1.0, 1.5, 2.0])
ax.set_yticklabels(["0.25","0.5","0.75","1.0","1.5","2.0"], fontsize=7, color="#555")
ax.axhline(y=1.0, color="#999", linewidth=1, linestyle="--")  # baseline reference

ax.legend(loc="upper right", bbox_to_anchor=(1.38, 1.15), fontsize=9)
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig3_radar_ft.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig3_radar_ft.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig3_radar_ft.pdf/.svg")

# =============================================================================
# FIGURE 4 — Scheduler Effect: TP delta % by solver (closest vs random)
# =============================================================================
print("Building Figure 4: Scheduler Effect …")

fig, axes = plt.subplots(1, 2, figsize=(14, 5))
fig.suptitle("Scheduler Effect: Closest vs Random\n(Warehouse-L, 40 agents)", fontsize=12)

# Left: TP delta % by solver × scenario
pivot_tp_delta = sched_df.pivot(index="solver", columns="scenario", values="tp_delta_pct")
pivot_tp_delta = pivot_tp_delta.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)
pivot_tp_delta_labeled = pivot_tp_delta.rename(
    index=SOLVER_LABELS, columns=SCENARIO_LABELS
)

x   = np.arange(len(SOLVER_ORDER))
w   = 0.11
ax  = axes[0]
for i, (scen, scen_label) in enumerate(SCENARIO_LABELS.items()):
    vals = pivot_tp_delta_labeled[scen_label].values
    ax.bar(x + i*w - (len(SCENARIO_ORDER)-1)*w/2, vals, w,
           label=scen_label, alpha=0.85)
ax.axhline(0, color="#333", linewidth=0.8)
ax.set_xticks(x)
ax.set_xticklabels(list(SOLVER_LABELS.values()), rotation=25, ha="right", fontsize=9)
ax.set_ylabel("TP delta % (closest − random)", fontsize=9)
ax.set_title("Throughput Lift: Closest vs Random Scheduler", fontsize=10)
ax.legend(fontsize=7, loc="upper right")
ax.grid(axis="y", alpha=0.3)

# Right: FT delta (closest - random) heatmap
pivot_ft_delta = sched_df.pivot(index="solver", columns="scenario", values="ft_delta")
pivot_ft_delta = pivot_ft_delta.reindex(index=SOLVER_ORDER, columns=SCENARIO_ORDER)

cmap2 = plt.cm.RdBu
norm2 = mcolors.TwoSlopeNorm(vmin=-0.2, vcenter=0.0, vmax=0.2)
im2 = axes[1].imshow(pivot_ft_delta.values, cmap=cmap2, norm=norm2, aspect="auto")
for i in range(len(SOLVER_ORDER)):
    for j in range(len(SCENARIO_ORDER)):
        v = pivot_ft_delta.values[i, j]
        if not np.isnan(v):
            color = "white" if abs(v) > 0.15 else "black"
            axes[1].text(j, i, f"{v:+.3f}", ha="center", va="center", fontsize=8, color=color)
axes[1].set_xticks(range(len(SCENARIO_ORDER)))
axes[1].set_xticklabels([SCENARIO_LABELS[s] for s in SCENARIO_ORDER],
                         rotation=35, ha="right", fontsize=8)
axes[1].set_yticks(range(len(SOLVER_ORDER)))
axes[1].set_yticklabels([SOLVER_LABELS[s] for s in SOLVER_ORDER], fontsize=9)
axes[1].set_title("FT Delta (closest − random)", fontsize=10)
plt.colorbar(im2, ax=axes[1], shrink=0.8, label="ΔFT")

plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig4_scheduler_effect.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig4_scheduler_effect.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig4_scheduler_effect.pdf/.svg")

# =============================================================================
# FIGURE 5 — Critical Time comparison by solver and fault category
# =============================================================================
print("Building Figure 5: Critical Time by Fault Category …")

wl40["fault_cat"] = wl40.scenario.map(FAULT_CATS)
ct_cat = (
    wl40.groupby(["solver","fault_cat"])["critical_time_mean"]
    .mean()
    .reset_index()
)

fig, ax = plt.subplots(figsize=(10, 5))
cats = ["Recoverable", "Permanent"]
x    = np.arange(len(SOLVER_ORDER))
w    = 0.3
cat_colors = {"Recoverable": "#4CAF50", "Permanent": "#FF9800"}

for i, cat in enumerate(cats):
    vals = []
    for s in SOLVER_ORDER:
        row = ct_cat[(ct_cat.solver == s) & (ct_cat.fault_cat == cat)]
        vals.append(row.critical_time_mean.values[0] if not row.empty else np.nan)
    ax.bar(x + i*w - w/2, vals, w, label=cat, color=cat_colors[cat], alpha=0.85)

ax.set_xticks(x)
ax.set_xticklabels([SOLVER_LABELS[s] for s in SOLVER_ORDER], rotation=20, ha="right", fontsize=9)
ax.set_ylabel("Mean Critical Time (fraction of ticks below 50% baseline)", fontsize=9)
ax.set_title("Solver Resilience: Critical Time by Fault Category\n(Warehouse-L, n=40)", fontsize=11)
ax.legend(fontsize=9)
ax.set_ylim(0, 0.65)
ax.grid(axis="y", alpha=0.3)
ax.axhline(0.5, color="#c00", linewidth=1, linestyle=":", label="50% threshold")
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig5_critical_time.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig5_critical_time.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig5_critical_time.pdf/.svg")

# =============================================================================
# FIGURE 6 — Braess Paradox: FT > 1 scatter (x=survival_rate, y=FT, bubble=density)
# =============================================================================
print("Building Figure 6: Braess Paradox scatter …")

braess_plot = all_e1[all_e1.ft_mean > 1.0].copy()
braess_plot["solver_label"] = braess_plot.solver.map(SOLVER_LABELS)
braess_plot["scen_label"]   = braess_plot.scenario.map(SCENARIO_LABELS)
braess_plot["cat"]          = braess_plot.scenario.map(FAULT_CATS)

fig, ax = plt.subplots(figsize=(11, 7))
cat_markers = {"Recoverable": "^", "Permanent": "o"}
cat_edge    = {"Recoverable": "#1B5E20", "Permanent": "#E65100"}

for _, row in braess_plot.iterrows():
    cat_val = row["cat"]
    solver_val = row["solver"]
    marker = cat_markers.get(cat_val, "o")
    color  = SOLVER_COLORS.get(solver_val, "#888")
    size   = 80 + row["num_agents"] * 1.5
    ax.scatter(row["survival_rate_mean"], row["ft_mean"],
               s=size, marker=marker, color=color,
               edgecolors=cat_edge.get(cat_val,"#333"), linewidths=1.2, alpha=0.85)
    ax.annotate(f"{row['solver_label']}\nn={int(row['num_agents'])}",
                (row["survival_rate_mean"], row["ft_mean"]),
                fontsize=6.5, xytext=(4, 2), textcoords="offset points", color="#333")

ax.axhline(1.0, color="#555", linewidth=1.2, linestyle="--", label="FT = 1.0 (baseline)")
ax.axhline(1.5, color="#f00", linewidth=0.8, linestyle=":", alpha=0.6, label="FT = 1.5")
ax.set_xlabel("Survival Rate (fraction of fleet alive)", fontsize=10)
ax.set_ylabel("Fault Tolerance (FT = P_fault / P_baseline)", fontsize=10)
ax.set_title("Braess Paradox: Configurations where fault-induced removal IMPROVES throughput\n(bubble size ∝ fleet density)", fontsize=11)

# Legend: solver colors
solver_patches = [Patch(color=SOLVER_COLORS[s], label=SOLVER_LABELS[s]) for s in SOLVER_ORDER]
cat_patches = [Patch(color=cat_edge[c], label=c) for c in cat_markers]
leg1 = ax.legend(handles=solver_patches, title="Solver", fontsize=8, loc="upper left", framealpha=0.9)
ax.add_artist(leg1)
ax.legend(handles=cat_patches, title="Fault Category", fontsize=8, loc="upper right", framealpha=0.9)

ax.grid(True, alpha=0.3)
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig6_braess_scatter.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig6_braess_scatter.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig6_braess_scatter.pdf/.svg")

# =============================================================================
# FIGURE 7 — Fleet Utilization by solver × fault category
# =============================================================================
print("Building Figure 7: Fleet Utilization by fault category …")

wl40["fault_cat"] = wl40.scenario.map(FAULT_CATS)
fu_cat = (
    wl40.groupby(["solver","fault_cat"])["fleet_utilization_mean"]
    .mean()
    .reset_index()
)

fig, axes7 = plt.subplots(1, 2, figsize=(14, 5))
fig.suptitle("Fleet Utilization Post-Fault by Solver\n(Warehouse-L, n=40)", fontsize=12)

# Left: grouped bars (fault category × solver)
cats7 = ["Recoverable", "Permanent"]
x7   = np.arange(len(SOLVER_ORDER))
w7   = 0.3
cat_colors7 = {"Recoverable": "#4CAF50", "Permanent": "#FF9800"}

ax7l = axes7[0]
for i, cat in enumerate(cats7):
    vals = []
    for s in SOLVER_ORDER:
        row = fu_cat[(fu_cat.solver == s) & (fu_cat.fault_cat == cat)]
        vals.append(row.fleet_utilization_mean.values[0] if not row.empty else np.nan)
    ax7l.bar(x7 + i*w7 - w7/2, vals, w7, label=cat, color=cat_colors7[cat], alpha=0.85)

ax7l.set_xticks(x7)
ax7l.set_xticklabels([SOLVER_LABELS[s] for s in SOLVER_ORDER], rotation=20, ha="right", fontsize=9)
ax7l.set_ylabel("Mean Fleet Utilization (alive+tasked / initial fleet)", fontsize=9)
ax7l.set_title("Fleet Utilization by Fault Category", fontsize=10)
ax7l.legend(fontsize=9)
ax7l.set_ylim(0, 1.1)
ax7l.axhline(1.0, color="#555", linewidth=0.8, linestyle="--", alpha=0.6)
ax7l.grid(axis="y", alpha=0.3)

# Right: cascade spread heatmap (solver × scenario)
ax7r = axes7[1]
cmap7 = plt.cm.YlOrRd
im7 = ax7r.imshow(cascade_spread_pivot.values, cmap=cmap7, aspect="auto", vmin=0)
for i in range(len(SOLVER_ORDER)):
    for j in range(len(SCENARIO_ORDER)):
        v = cascade_spread_pivot.values[i, j]
        if not np.isnan(v):
            color = "white" if v > cascade_spread_pivot.values[~np.isnan(cascade_spread_pivot.values)].max() * 0.65 else "black"
            ax7r.text(j, i, f"{v:.1f}", ha="center", va="center", fontsize=8, color=color)
ax7r.set_xticks(range(len(SCENARIO_ORDER)))
ax7r.set_xticklabels([SCENARIO_LABELS[s] for s in SCENARIO_ORDER], rotation=35, ha="right", fontsize=8)
ax7r.set_yticks(range(len(SOLVER_ORDER)))
ax7r.set_yticklabels([SOLVER_LABELS[s] for s in SOLVER_ORDER], fontsize=9)
ax7r.set_title("Cascade Spread (avg agents affected per fault event)", fontsize=10)
plt.colorbar(im7, ax=ax7r, shrink=0.8, label="agents")

plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig7_fleet_utilization.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig7_fleet_utilization.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig7_fleet_utilization.pdf/.svg")

# =============================================================================
# FIGURE 8 — Solver compute cost vs FT (bubble = throughput)
# =============================================================================
print("Building Figure 8: Compute cost vs FT trade-off …")

wl40_mean = (
    wl40.groupby("solver")
    .agg(
        ft_mean=("ft_mean","mean"),
        tp_mean=("throughput_mean","mean"),
        us_mean=("solver_step_us_mean","mean"),
        ct_mean=("critical_time_mean","mean"),
    )
    .reset_index()
)

fig, ax = plt.subplots(figsize=(9, 6))
for _, row in wl40_mean.iterrows():
    ax.scatter(row.us_mean, row.ft_mean,
               s=max(30, row.tp_mean * 800), alpha=0.82,
               color=SOLVER_COLORS.get(row.solver,"#888"),
               edgecolors="#333", linewidths=1)
    ax.annotate(SOLVER_LABELS[row.solver],
                (row.us_mean, row.ft_mean),
                fontsize=9, xytext=(6, 3), textcoords="offset points")

ax.set_xscale("log")
ax.set_xlabel("Mean solver step time (μs, log scale)", fontsize=10)
ax.set_ylabel("Mean Fault Tolerance (FT) across all scenarios", fontsize=10)
ax.set_title("Compute Cost vs Fault Resilience\n(bubble size ∝ mean throughput, Warehouse-L n=40)", fontsize=11)
ax.axhline(1.0, color="#888", linewidth=1, linestyle="--")
ax.grid(True, alpha=0.3)
plt.tight_layout()
plt.savefig(os.path.join(FIG_DIR, "fig8_cost_vs_ft.pdf"), dpi=150, bbox_inches="tight")
plt.savefig(os.path.join(FIG_DIR, "fig8_cost_vs_ft.svg"), bbox_inches="tight")
plt.close()
print("  → figures/fig8_cost_vs_ft.pdf/.svg")

# =============================================================================
# TEXT REPORT
# =============================================================================
print("\nBuilding text report …")

n_braess = len(braess_table)
braess_solvers = braess_table["Solver"].value_counts().to_dict()
top_ft = ft_pivot.stack().dropna()
top_ft_row = top_ft.idxmax()
max_ft_val = top_ft.max()

# Best FT per scenario (averaged across topologies at default density)
best_by_scen = t1_agg.loc[t1_agg.groupby("scenario")["ft_mean"].idxmax()][["scenario","solver","ft_mean"]]

# Mean TP closest vs random
tp_lift_overall = sched_df.tp_delta_pct.mean()
ft_lift_overall = sched_df.ft_delta.mean()

report = f"""
MAFIS PAAMS 2026 — Experiment Report
=====================================
Run date:    2026-04-07
Total runs:  7,920 (main_matrix, 4 faithful solvers, 6 scenarios)
Tick count:  500 per simulation
Topologies:  warehouse_large (57×33), kiva_warehouse (48×48), compact_grid (26×26)
Solvers:     4  (PIBT, RHCR-PBS, Token-P, LaCAM3) — fidelity-audited solver-refocus set
Scenarios:   6  (burst-20%, burst-50%, wear-med, wear-high, zone-out, intermittent-80s80m15r)
Scheduler:   closest (E1), closest+random (E2)

═══════════════════════════════════════════
RQ1 — SOLVER RESILIENCE
═══════════════════════════════════════════

FT (mean across topologies, default density):
{ft_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).to_string()}

Key findings:
• Peak Braess FT: {max_ft_val:.3f} — solver {SOLVER_LABELS.get(top_ft_row[0], top_ft_row[0])}, scenario {SCENARIO_LABELS.get(top_ft_row[1], top_ft_row[1])}
• Total Braess cases (FT > 1.0): {n_braess}
  By solver: {braess_solvers}

Best FT per scenario (avg across topologies):
{best_by_scen.to_string(index=False)}

Critical Time (fraction of ticks below 50% baseline, warehouse_large n=40):
{ct_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).round(3).to_string()}

Cascade Spread (avg agents affected per fault event, warehouse_large n=40):
{cascade_spread_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).round(2).to_string()}

Fleet Utilization (alive+tasked / initial_fleet post-fault, warehouse_large n=40):
{fleet_util_pivot.rename(index=SOLVER_LABELS, columns=SCENARIO_LABELS).round(3).to_string()}

═══════════════════════════════════════════
RQ2 — TOPOLOGY EFFECT
═══════════════════════════════════════════

FT (PIBT, default density per topology):
{topo_pivot.rename(columns=SCENARIO_LABELS).round(3).to_string()}

═══════════════════════════════════════════
RQ3 — SCALE SENSITIVITY (PIBT)
═══════════════════════════════════════════

Mean FT by density:
{pibt_e1.groupby(['topology','num_agents'])['ft_mean'].mean().unstack().round(3).to_string()}

═══════════════════════════════════════════
RQ4 — SCHEDULER EFFECT (warehouse_large, n=40)
═══════════════════════════════════════════

Overall closest vs random:
  Mean TP lift (closest − random):  {tp_lift_overall:+.2f}%
  Mean FT delta (closest − random): {ft_lift_overall:+.4f}

Per-solver mean TP lift:
{sched_tp_summary.to_string()}

Per-solver mean FT delta:
{sched_ft_summary.to_string()}

═══════════════════════════════════════════
RQ5 — BRAESS PARADOX
═══════════════════════════════════════════

{braess_table[['Solver','Topo','N','Scenario','Category','FT','CI_lo','CI_hi']].to_string(index=False)}

═══════════════════════════════════════════
OUTPUT FILES
═══════════════════════════════════════════

Tables (results/tables/):
  table1_ft.csv / .tex         — Solver × Scenario FT
  table1_critical_time.csv/.tex— Solver × Scenario Critical Time
  table2_topology_ft.csv/.tex  — Topology × Density FT (PIBT)
  table3_scheduler_*.csv       — Scheduler effect summary + TP delta pivot
  table4_braess_cases.csv/.tex — All Braess paradox instances

Figures (results/figures/):
  fig1_ft_heatmap.*            — FT heatmap (3 topologies)
  fig2_throughput_density.*    — TP vs fleet size (3 topologies)
  fig3_radar_ft.*              — Radar: solver FT across scenarios
  fig4_scheduler_effect.*      — TP delta % + FT delta heatmap
  fig5_critical_time.*         — Critical time by fault category
  fig6_braess_scatter.*        — Braess cases scatter
  fig7_fleet_utilization.*     — Fleet utilization by fault category + cascade spread heatmap
  fig8_cost_vs_ft.*            — Compute cost vs FT trade-off
"""

with open(os.path.join(RESULTS, "report.txt"), "w") as f:
    f.write(report)
print(report)
print("\n✓ All outputs written.")
print(f"  {FIG_DIR}  (8 figures × PDF+SVG)")
print(f"  {TAB_DIR}  (4 tables × CSV+LaTeX)")
print(f"  {RESULTS}/report.txt")
