# scripts/analysis — MAFIS analysis pipeline

Canonical home for every Python script that consumes MAFIS experiment outputs and produces figures, tables, or statistical summaries. `results/` holds data only; this directory holds code.

## Shared libraries

| File | Purpose |
|------|---------|
| `stats.py` | Mann–Whitney U, Cliff's δ, Benjamini–Hochberg FDR, 95% Student-t CI, paired comparison helpers |
| `constants.py` | Scenario, solver, and topology labels plus colour palettes used across all figures |

## Paper pipeline

| Script | Inputs | Outputs |
|--------|--------|---------|
| `paper_figures.py` | `results/*_summary.csv`, `results/all_runs.csv` | PAAMS 2026 paper figures (FT heatmaps, AR heatmaps, SVG/PDF exports) |
| `tail_metrics.py` | `results/all_runs.csv` | `results/tails.csv` — CVaR α=0.8 + Top-Event Probability |
| `fdr_pairwise.py` | `results/*_summary.csv` (multiple) | `results/fdr_adjusted_pvalues.csv` — Welch's t + Benjamini–Hochberg corrected p-values |
| `metric_correlation.py` | `results/*_summary.csv` | Correlation matrix (Pearson + Spearman) PNG + JSON |
| `topology_sensitivity.py` | `results/*_summary.csv` | Rank-flip table and shift/noise PNGs across topologies |
| `speed_robustness.py` | `results/*_summary.csv` | Scatter of solver step time vs FT |

## Aisle-width sweep (RQ2 / Appendix B)

| Script | Purpose |
|--------|---------|
| `rhcr_braess_observatory_proof.py` | PBS partial-rate ↔ FT correlation proof |
| `structural_cascade_scaling.py` | Structural cascade vs walkable area and aisle width |
| `mitigation_delta.py` | Solver mitigation skill (`cascade_spread − structural_cascade`) |
| `ft_baseline_audit.py` | FT validity flags — overloaded baseline detection |
| `delta_diff.py` | Pre/post kick-back-fix drift table |

## Supplementary

| Script | Purpose |
|--------|---------|
| `scheduler_effect_analysis.py` | Closest-first vs random scheduler comparison. Feeds the supplementary scheduler post; not used in the paper body |

## Data curation (one-off)

| Script | Purpose |
|--------|---------|
| `data_merge_post_kickback_fix.py` | Merge pre-fix + post-fix aisle-width CSVs after the queue kick-back fix |
| `data_merge_tp_rerun.py` | Merge Token Passing re-run into the original aisle-width sweep |

## Figures

`figures/fig5_rq1.py` and `figures/fig6_rq2.py` regenerate the two paper figures from `all_runs.csv`. Compiled PDFs live next to `main.tex` in `docs/papers/paper1_drafts/paams2026/figures/`.

## Archive

Scripts in `archive/` ran on data that no longer exists in `results/` (pre-aisle-width experiment outputs were purged). They are kept as a methodological record:

- `braess_analysis.py` — Mann-Whitney + Cliff's δ on early Braess data
- `scale_sensitivity_analysis.py` — early RQ3 scale sensitivity
- `solver_resilience_analysis.py` — early RQ1 FT-per-solver heatmap
- `topology_effect_analysis.py` — early RQ2 FT-per-topology heatmap
- `ct_rolling_audit.py` — one-time audit of CT pre/post rolling-mean fix (2026-04-10)

Restore raw CSVs from git history if you need to re-run any of these.

## Running a script

```bash
cd <repo-root>
python scripts/analysis/<script>.py
```

Most scripts read CSVs from `results/` relative to the repo root. Some write back into `results/` (`fdr_pairwise.py`, `tail_metrics.py`) and others write into `docs/papers/paper1_drafts/paams2026/figures/` (`paper_figures.py`, `figures/fig*.py`).
