# MAFIS Supplementary Materials

Companion folder for the PAAMS 2026 paper "MAFIS: A Resilience Observatory for Lifelong Multi-Agent Path Finding". The paper body is intentionally narrow and presents headline findings only. Analyses that the paper either did not have room for or treats as second-order live here.

## Contents

| Folder | Headline |
|--------|----------|
| [`topology_density_sensitivity/`](topology_density_sensitivity/) | At the only shared density (n=40), switching warehouse map shifts mean FT by 0.07. Within each topology, density is a strong lever — see the per-scenario trajectories. Step sizes differ between topologies (SD steps +20 agents, DD steps +40), so cross-topology density-step comparison is not apples-to-apples. |
| [`scheduler_effect/`](scheduler_effect/) | Closest-first vs random scheduler at SD-w1 n=40: mean FT 0.673 vs 0.662 across the six scenarios. Only one scenario (wear_high) is significant after BH-FDR, and its FT delta is +0.004. |
| [`aisle_width_proof/`](aisle_width_proof/) | The paradoxical FT > 1 regime under RHCR-PBS is NOT caused by PBS node-budget saturation. Pooled correlation between partial-rate and FT is r = −0.029, p = 0.51 (n = 518). Actual mechanism remains open. |

## Data files

Each subfolder ships with its own raw CSVs in the folder itself. Top-level:
- [`fdr_adjusted_pvalues.csv`](fdr_adjusted_pvalues.csv) — Benjamini–Hochberg-corrected p-values for every pairwise solver test (Welch's t per cell, five metrics).

## Mirror

A reader-friendly narrative version of these analyses appears on the project blog at <https://stasis-website.vercel.app/blog>. The blog mirrors the same findings without the citation overhead.

## Cite

If you use any of these materials, cite the PAAMS 2026 paper.
