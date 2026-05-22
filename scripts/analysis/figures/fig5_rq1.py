import pandas as pd
import matplotlib.pyplot as plt
import numpy as np

df = pd.read_csv("/Users/teddyadmin/Developments/Research-Project/mafis/results/all_runs.csv")
f = df[df["is_baseline"] == False]

# warehouse_large only, N=40
wl = f[(f["topology"] == "warehouse_large") & (f["num_agents"] == 40)]
g = wl.groupby(["scenario", "solver"])["fault_tolerance"].mean().unstack("solver")

scen_order = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high", "zone_50t", "intermittent_80s80m15r"]
g = g.reindex(scen_order)

# Nicer x-tick labels
scen_labels = ["Burst 20%", "Burst 50%", "Wear Med.", "Wear High", "Zone 50t", "Intermit."]

fig, ax = plt.subplots(figsize=(8, 3.5))
g.plot(kind="bar", ax=ax, width=0.75, edgecolor="black")
ax.axhline(1.0, color="gray", linestyle="--", linewidth=0.8, label="Baseline (FT=1)")
ax.set_xlabel("Fault scenario")
ax.set_ylabel("Fault Tolerance (mean over seeds)")
ax.set_title("RQ1: Solver vulnerability profile (warehouse\\_large, N=40)")
ax.legend(title="Solver", loc="upper right", fontsize=8)
ax.set_xticklabels(scen_labels, rotation=30, ha="right", fontsize=8)
plt.tight_layout()
plt.savefig(
    "/Users/teddyadmin/Developments/Research-Project/mafis/docs/papers/paper1_drafts/paams2026/figures/fig5_rq1_vulnerability.pdf",
    bbox_inches="tight",
)
print("Fig 5 saved")
