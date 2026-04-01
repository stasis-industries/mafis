# Reproducing MAFIS Results

This document describes how to reproduce the experimental results for the MAFIS research
project — a fault resilience observatory for lifelong multi-agent path finding.
The manuscript is in preparation.

## Requirements

- **Rust** 1.75+ (tested on 1.93.0)
- **Python** 3.8+ (tested on 3.14.3), stdlib only, no pip packages needed
- ~4 GB RAM for full experiment matrix
- ~2 GB disk for compiled binaries + results

---

## Quick Verification (~3 min)

```bash
cargo check        # Type check (~5s)
cargo test         # Full test suite (~3 min)
```

This runs all unit tests, integration tests, metamorphic tests, calibration tests,
benchmark tests, desktop tests, and a smoke test. All should pass with 0 failures.

---

## Property Verification (~4 min)

The calibration tests verify known algorithmic properties:

```bash
cargo test --release --test calibration -- --nocapture
```

Six property tests:
1. **Throughput saturation** — per-agent throughput decreases at high density (Okumura 2022)
2. **PIBT completeness** — no deadlock on any topology
3. **Liveness at high density** — PIBT produces tasks even at n=70 on dense grids
4. **All solvers functional** — all 7 solvers produce positive throughput
5. **Topology sensitivity** — different maps produce different throughput profiles
6. **Differential measurement** — deterministic baselines + faults have measurable effect

---

## Paper 1: Full Experiment Matrix (~1-2 hours)

```bash
cargo test --release full_paper_matrix -- --ignored --nocapture
```

Runs 3,570 experiment configurations × 2 (baseline + fault) = **7,140 simulations**
across four sub-experiments:

| Sub-experiment     | Independent variable | Configs  |
|--------------------|----------------------|----------|
| Solver resilience  | 6 solvers            | 1,260    |
| Scale sensitivity  | 4 fleet sizes        |   840    |
| Scheduler effect   | 2 schedulers         |   420    |
| Topology effect    | 5 warehouse layouts  | 1,050    |

All experiments are controlled on a single axis; all other variables are fixed.
Results are written to `results/`:
- `*_runs.csv` — per-run metrics (one row per seed)
- `*_summary.csv` — aggregated statistics (mean, CI95 per group)
- `*.json` — structured output for programmatic access

Individual sub-experiments can also be run separately:

```bash
cargo test --release solver_resilience -- --ignored --nocapture
cargo test --release scale_sensitivity -- --ignored --nocapture
cargo test --release scheduler_effect  -- --ignored --nocapture
cargo test --release topology_medium   -- --ignored --nocapture  # warehouse_large
cargo test --release topology_large    -- --ignored --nocapture  # kiva_warehouse
```

---

## Paper 1: Statistical Analysis (~1s)

Four analysis scripts, one per sub-experiment. Each produces a CSV of grouped
statistics, a LaTeX table, and an SVG visualization.

```bash
python3 analysis/solver_resilience_analysis.py   # heatmap: solver × scenario
python3 analysis/scale_sensitivity_analysis.py   # curves: FT ratio vs fleet size
python3 analysis/scheduler_effect_analysis.py    # bars: random vs closest per scenario
python3 analysis/topology_effect_analysis.py     # heatmap: topology × scenario
```

All scripts use only Python stdlib (no numpy/scipy/matplotlib).
Outputs are written to `results/`:

| Script                         | CSV output                     | LaTeX                        | SVG                             |
|--------------------------------|--------------------------------|------------------------------|---------------------------------|
| solver_resilience_analysis.py  | solver_resilience_metrics.csv  | solver_resilience_table.tex  | solver_resilience_heatmap.svg   |
| scale_sensitivity_analysis.py  | scale_sensitivity_metrics.csv  | scale_sensitivity_table.tex  | scale_sensitivity_curves.svg    |
| scheduler_effect_analysis.py   | scheduler_effect_metrics.csv   | scheduler_effect_table.tex   | scheduler_effect_contrast.svg   |
| topology_effect_analysis.py    | topology_effect_metrics.csv    | topology_effect_table.tex    | topology_effect_heatmap.svg     |

Statistical methodology: paired Mann-Whitney U test per group, Benjamini-Hochberg FDR
correction across all tests within each sub-experiment, Cliff's delta effect size.

---

## One-Command Reproduction

```bash
sh scripts/reproduce.sh
```

Runs the complete pipeline: type check → tests → calibration → Paper 1 experiments
→ Paper 1 analysis.

---

## Paper 2: Braess Experiment (~2-4 additional hours)

The Braess analysis tests whether fault-induced agent removal paradoxically improves
throughput at high density. Reserved for a follow-up study.

```bash
# Main experiment: 7 solvers × 4 densities × 6 scenarios × 50 seeds
cargo test --release braess_resilience -- --ignored --nocapture

# Category 3 complement (permanent zone outage, 5 solvers × 4 densities × 50 seeds)
cargo test --release run_braess_perm_zone -- --ignored --nocapture

# Analysis (reads both CSVs if both are present)
python3 analysis/braess_analysis.py
```

Outputs: `braess_ratios.csv`, `braess_significance.csv`, `braess_table.tex`,
`braess_degradation.svg`.

---

## Exploratory / Validation Experiments

These experiments are not part of the main pipeline. Run them individually as needed.

**Cross-topology Braess validation** (480 runs):  
Tests whether the Braess effect replicates on sorting_center and compact_grid.
```bash
cargo test --release run_cross_topology -- --ignored --nocapture
```
Output: `results/cross_topology_runs.csv`

**New solver resilience variant** (540 runs):  
Closest scheduler, n=20, 3 representative fault scenarios — exploratory follow-up.
```bash
cargo test --release run_new_solver_resilience -- --ignored --nocapture
```
Output: `results/new_solver_resilience_runs.csv`

**Solver benchmark** (~70 runs):  
Baseline throughput comparison across all 7 solvers at n=40.
```bash
cargo test --release run_solver_benchmark -- --ignored --nocapture
```
Output: `results/solver_benchmark_runs.csv`

---

## Expected Runtimes

| Step                              | Time          | Hardware           |
|-----------------------------------|---------------|--------------------|
| `cargo check`                     | ~5s           | Any                |
| `cargo test`                      | ~3 min        | Any                |
| Calibration tests                 | ~4 min        | Any                |
| Paper 1 full matrix               | ~1–2 hours    | 8+ cores (Rayon)   |
| Paper 1 analysis (4 scripts)      | ~2s total     | Any                |
| Braess experiment (Paper 2)       | ~2–4 hours    | 8+ cores           |
| Braess analysis                   | ~1s           | Any                |

---

## Determinism

All simulations are deterministic: same seed + same config = identical output.
The paired experimental design (baseline vs. faulted run) uses the same seed for both,
so metric differences are causally attributable to the fault condition.

---

## Solver Fidelity

See `docs/solver_fidelity.md` for a per-solver audit of algorithmic accuracy,
documented deviations, and test coverage.
