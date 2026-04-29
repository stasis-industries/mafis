# MAFIS Reliability Checklist

**Purpose**: continuous audit of the artefacts that experiment results, public claims, and downstream consumers depend on. **Nothing advances to the next research phase until the items it depends on have a green status here.**

Legend: green — audited, stable, claim-safe · yellow — known limitation, must be disclosed alongside any claim · red — unresolved issue, do not use for claims

---

## Metrics

| Metric | Status | Notes |
|---|---|---|
| Fault Tolerance (FT) | yellow | Ratio $P_{\text{fault}}/P_{\text{baseline}}$. Mathematically unstable when baseline near zero — runs require a baseline-validity preprocessing step; overloaded cells excluded from default-density aggregates. The queue kick-back fix (commit `2ee0313`) removed a baseline-deflation source. |
| Critical Time (CT) | green | Threshold $=0.5 \times P_{\text{baseline}}$, rolling window $W{=}10$. |
| ITAE | yellow | Correct integration. Unbounded if baseline has near-zero ticks; downstream consumers must clamp or filter degenerate baselines. |
| Rapidity | green | Degradation-observed gate inside `compute_rapidity`. Smoothing window $W{=}20$. |
| Attack Rate (AR) | green | Denominator = `actual_agents` (post-grid-clamp). MAX_CASCADE_DEPTH=200 and ADG_LOOKAHEAD=3 declared in `src/constants.rs`. Saturates at 1.00 on Intermittent by design (repeated waves), not a measurement artefact. |
| Cascade Depth | green | Mean of per-event BFS max depth. Hard cap raised 10 → 200 in commit `2340eac` (see `src/constants.rs:82`); non-binding for the evaluated topologies. Tautologically 0 for permanent faults (dead agents create independent obstacles, not dependency chains). |
| `rhcr_partial_rate` (observatory probe) | green | Per-run fraction of PBS planning windows returning `WindowResult::Partial` (LRA + PIBT fallback). 3 behavioural tests in `src/solver/rhcr/solver.rs`. |

---

## Fault models

| Scenario | Status | Parameter provenance | Real-world analog | Notes |
|---|---|---|---|---|
| Burst-20%, Burst-50% | yellow | Pure design choice, no literature | "Motor failure" analog | Simultaneous $N\%$ loss is not a single-mechanism failure; document as stress-test |
| Wear-Medium (β=2.5, η=500) | yellow | Uncited industry survey in code comment | "Battery degradation" analog | η is sim-window tuning knob; hour→tick conversion still missing |
| Wear-High (β=3.5, η=150) | yellow | Carlson & Murphy 2005 cited in code | "Battery degradation" analog | Only scenario with literature cite; unit conversion still missing |
| Zone-Out (50-tick strip) | yellow | Pure design choice | "Network partition" analog | Arbitrary strip geometry and 2.5-sec recovery; document as spatial-disruption stress-test |
| Intermittent (MTBF=80, rec=15) | yellow | Pure design choice | "Sensor glitch" analog | Memoryless assumption contradicts real clustering; document as tractability-simplified model |

---

## Solvers

| Solver | Status | Notes |
|---|---|---|
| PIBT | green | `docs/solver_fidelity.md` — no documented deviations |
| RHCR-PBS | green | `docs/solver_fidelity.md` — historical deviations closed, 1 deliberate remains (deterministic node budget). Override path (`RhcrConfigOverride` + factory `lifelong_solver_from_name_with_override`) is non-invasive: passes through `RhcrConfig::auto` by default, covered by 2 `rhcr_override_*` unit tests. |
| Token Passing | green | `docs/solver_fidelity.md` — 3 tracked deviations, all semantically equivalent or MAFIS-specific. Goal-change sync + rewind determinism fix in commit `845a6fb`. |

---

## Determinism + reproducibility

| Property | Status | Notes |
|---|---|---|
| Bit-identical replay within a machine | green | ChaCha8 RNG, no wall-clock in RNG chain |
| Cross-machine bit-identity | yellow | Rayon parallel reductions + f64 summation order may differ by ULP. Disclose as "bit-identical traces within a machine" with a note on cross-machine ULP drift. |
| Paired-baseline RNG isolation | green | `fault_rng` stream is isolated. Task RNG is shared up to $t_f$ and burst-fault events consume task RNG by construction, introducing deterministic divergence at $t_f$. |
| Baseline caching correctness | green | Regression tests in `src/experiment/runner.rs` confirm parity vs uncached runs. `BaselineKey` includes `rhcr_override_label()` so override variants don't collide. |
| RHCR override / ablation determinism | green | 2 unit tests in `src/solver/mod.rs` factory_tests (`rhcr_override_applied`, `rhcr_override_none_keeps_auto`); 3 behavioural tests for `pbs_partial_rate` lifecycle in `src/solver/rhcr/solver.rs`. |

---

## Experiment pipeline

| Area | Status | Notes |
|---|---|---|
| `run_single_experiment` | green | Standalone paired run, tested |
| `run_matrix` + rayon parallelism | green | Parallel-safe; per-config deterministic given shared baseline key |
| CSV/JSON export | green | Schema stable + extended with `pbs_partial_rate` column. Downstream Python reads by name, not index. |

---

## Quality gate — adding a new solver

Before merging a new solver:

1. Implements `LifelongSolver` trait correctly (`step()` returns `Replan` or `Continue`).
2. Factory entry in `lifelong_solver_from_name` (`src/solver/mod.rs`).
3. Determinism: same seed → same trace. Add a regression test mirroring existing solver tests.
4. Fidelity note: add an entry to `docs/solver_fidelity.md` listing any deviations from the canonical reference (PIBT, RHCR, Token Passing source papers).
5. Behavioural test: at least one test exercising the solver against a small canonical scenario.
6. Update the solver table in `CLAUDE.md` and the metric audit row above.

---

## Quality gate — adding a new fault model

Before merging a new fault scenario:

1. Cite the empirical or design provenance in the scenario's status row above (or mark yellow with a clear "design-choice" disclosure).
2. Determinism: fault sampling uses `fault_rng`, never wall-clock or task RNG.
3. Add a regression test under `tests/` that locks in the fault timing for a fixed seed.
4. Document the real-world analog and known mismatches.
5. Update the fault table in `CLAUDE.md` and the fault model table above.

---

## Quality gate — adding a new metric

Before merging a new metric:

1. Define the formula and units in code-comment + add a row above with status, audit notes.
2. Edge cases: behaviour at empty baseline, no events, zero alive agents.
3. Bound or saturate explicitly when unbounded — note saturation behaviour.
4. Regression test on a small canonical run with hand-checked expected values.
5. CSV/JSON column added to export schema. Downstream consumers read by name.

---

## Quality gate — adding a new topology

Before merging a new topology:

1. JSON file in `topologies/` with required fields (grid, zones, queue_direction, number_agents).
2. BFS connectivity validation passes (`validate_connectivity`).
3. `sh topologies/build-manifest.sh` re-run to refresh `manifest.json` and sync to `web/`.
4. At least one experiment-pipeline run completes without errors.

---

## Pre-public-release checklist

Before tagging a public release:

- [ ] All "Paper / claim dependency" rows above are green or documented yellow with disclosed caveat.
- [ ] No red item is referenced by any externally-visible claim.
- [ ] Test count on a clean tree matches or exceeds the previous release baseline.
- [ ] `cargo check`, `cargo test`, and the WASM build all pass on a clean checkout.
- [ ] Reproducibility note (`REPRODUCIBILITY.md`) reflects the current pipeline.
