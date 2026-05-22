import pandas as pd
import matplotlib.pyplot as plt
import matplotlib.colors as mcolors
import numpy as np

df = pd.read_csv("/Users/teddyadmin/Developments/Research-Project/mafis/results/all_runs.csv")
f = df[df["is_baseline"] == False]

# Per (topology, scenario, num_agents) average AR across solvers+seeds
pivot = f.groupby(["scenario", "topology", "num_agents"])["attack_rate"].mean().reset_index()

scen_order = ["burst_20pct", "burst_50pct", "wear_medium", "wear_high", "zone_50t", "intermittent_80s80m15r"]
scen_labels = ["Burst 20%", "Burst 50%", "Wear Med.", "Wear High", "Zone 50t", "Intermit."]
topo_order = ["warehouse_single_dock", "warehouse_dual_dock"]
topo_labels = ["SD-w1", "DD"]

fig, axes = plt.subplots(2, 3, figsize=(10, 4.2), sharey=True)
cmap = plt.get_cmap("YlOrRd")

for i, (scenario, slabel) in enumerate(zip(scen_order, scen_labels)):
    ax = axes[i // 3, i % 3]
    sub = pivot[pivot["scenario"] == scenario]
    mat = sub.pivot(index="topology", columns="num_agents", values="attack_rate").reindex(topo_order)

    im = ax.imshow(mat.values, aspect="auto", cmap=cmap, vmin=0, vmax=1)

    # Annotate cells
    for r in range(mat.shape[0]):
        for c in range(mat.shape[1]):
            val = mat.values[r, c]
            if not np.isnan(val):
                color = "white" if val > 0.6 else "black"
                ax.text(c, r, f"{val:.2f}", ha="center", va="center", fontsize=8, color=color)

    ax.set_xticks(range(len(mat.columns)))
    ax.set_xticklabels([str(v) for v in mat.columns], fontsize=8)
    ax.set_yticks(range(len(topo_order)))
    ax.set_yticklabels(topo_labels, fontsize=8)
    ax.set_title(slabel, fontsize=9)
    ax.set_xlabel("Agents", fontsize=8)
    if i % 3 == 0:
        ax.set_ylabel("Topology", fontsize=8)

    if i == 5:
        cbar = fig.colorbar(im, ax=ax, fraction=0.046, pad=0.04)
        cbar.set_label("Attack Rate", fontsize=8)

plt.suptitle("RQ2: Attack Rate by topology × density × scenario", fontsize=11, y=1.02)
plt.tight_layout()
plt.savefig(
    "/Users/teddyadmin/Developments/Research-Project/mafis/docs/papers/paper1_drafts/paams2026/figures/fig6_rq2_ar_heatmap.pdf",
    bbox_inches="tight",
)
print("Fig 6 saved")
