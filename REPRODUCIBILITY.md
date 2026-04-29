# Reproducing MAFIS Results

MAFIS is a fault-resilience observatory for lifelong multi-agent path finding (MAPF).
Every faulted run is paired with a deterministic same-seed baseline so metric differences
are causally attributable to the fault condition.

---

## Requirements

- **Rust** 1.75+ (tested on 1.93.0)
- **Python** 3.8+ with `pandas`, `matplotlib`, `numpy`, `scipy` (required by analysis scripts)
- ~4 GB RAM for the full experiment matrix
- ~2 GB disk for compiled binaries + results

---

## Quick Verification (~3 min)

```bash
cargo check        # Type check (~5s)
cargo test         # Full test suite (~3 min)
```

Runs unit tests, integration tests, metamorphic tests, calibration tests, benchmark tests,
and a smoke test. All should pass with 0 failures.

---

## Full Experiment Suite (~7 hours)

```bash
cargo test --release --test experiment_suite full_experiment_suite -- --ignored --nocapture
```

Produces 4,320 paired runs across 3 sub-experiments.
`run_matrix` parallelises over N-1 rayon threads; wall time ~7 hours on a single laptop.

| Sub-experiment               | Config                                            | Runs  |
|------------------------------|---------------------------------------------------|-------|
| Warehouse Single-Dock        | 3 solvers × 6 scenarios × 3 counts × 30 seeds     | 1,620 |
| Warehouse Dual-Dock          | 3 solvers × 6 scenarios × 3 counts × 30 seeds     | 1,620 |
| Scheduler Effect             | 3 solvers × 6 scenarios × 2 schedulers × 30 seeds | 1,080 |

Output files (written to `results/`, see `OUTPUT_DIR` constant in `tests/experiment_suite.rs`):

| File | Contents |
|------|----------|
| `results/warehouse_single_dock_experiment_runs.csv` | Per-run metrics, Single-Dock |
| `results/warehouse_single_dock_experiment_summary.csv` | Aggregated stats, Single-Dock |
| `results/warehouse_dual_dock_experiment_runs.csv` | Per-run metrics, Dual-Dock |
| `results/warehouse_dual_dock_experiment_summary.csv` | Aggregated stats, Dual-Dock |
| `results/scheduler_effect_experiment_runs.csv` | Per-run metrics, Scheduler Effect |
| `results/scheduler_effect_experiment_summary.csv` | Aggregated stats, Scheduler Effect |
| `results/all_runs.csv` | All runs combined (both rows per paired run) |
| `results/*.json` | Structured output per sub-experiment |

---

## RHCR Braess Observatory Proof (~1 hour)

Tests whether the FT > 1.2 observations under recoverable faults arise from PBS node-budget
saturation. Ablates 5 horizon × node-limit corners at 3 flagged cells
(SD-w1 n=60, SD-w2 n=108, SD-w3 n=151), 20 seeds × 300 ticks each.
~600 paired runs total.

```bash
cargo test --release --lib run_rhcr_braess_observatory_proof -- --ignored --nocapture
```

Output: `results/aisle_width/rhcr_braess_observatory_proof/{matrix}_{runs,summary}.csv`.
The test is idempotent: completed matrices are skipped on resume.

Analysis:
```bash
python3 scripts/analysis/rhcr_braess_observatory_proof.py
```

---

## Statistical Analysis (~1s each)

Analysis scripts live in `scripts/analysis/`. Install dependencies first:

```bash
pip install pandas matplotlib numpy scipy
```

| Script | Purpose |
|--------|---------|
| `structural_cascade_scaling.py` | Structural cascade R² and slopes across aisle widths |
| `mitigation_delta.py` | Mitigation Δ (FT, CT, ITAE) by solver and aisle width |
| `ft_baseline_audit.py` | Baseline-validity flags — identifies overloaded cells |
| `delta_diff.py` | Pre/post-fix drift table (compares CSV pairs) |

Run each with:
```bash
python3 scripts/analysis/<script>.py
```

---

## Expected Runtimes

| Step | Time | Notes |
|------|------|-------|
| `cargo check` | ~5s | Any machine |
| `cargo test` | ~3 min | Any machine |
| `full_experiment_suite` | ~7 hours | Single laptop; rayon N-1 threads |
| RHCR Braess observatory proof | ~1 hour | 600 runs, 20 seeds × 300 ticks |
| Analysis scripts | ~1s each | Python stdlib |

---

## Determinism

All simulations use a ChaCha8 generator seeded per config. Paired runs (baseline + faulted)
share the same seed, so metric differences are causally attributable to the fault condition.
Traces are bit-identical within a single machine. Cross-machine traces may differ by one
floating-point ULP on parallel reductions; run all seeds on one machine for strict
reproducibility.

---

## Solver Fidelity

See `RELIABILITY.md` for the per-solver audit of algorithmic accuracy, documented
deviations from reference implementations, and test coverage gates.
