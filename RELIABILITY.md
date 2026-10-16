# MAFIS Reliability Checklist

**Purpose**: continuous audit of the artefacts that experiment results, public claims, and downstream consumers depend on. **Nothing advances to the next research phase until the items it depends on have a green status here.**

Legend: green — audited, stable, claim-safe · yellow — known limitation, must be disclosed alongside any claim · red — unresolved issue, do not use for claims

Last audit: 2026-10-16 (branch `fix/reliability-audit`). Noise-floor figures below come from 10-seed probes at SD-w1 n=40, 500 ticks, closest scheduler, unless stated.

---

## Frozen results

| Artefact | Status | Notes |
|---|---|---|
| `results/**` CSVs (E1, aisle-width, scheduler, probes) | red | Produced before the 2026-10 audit fixes (RNG streams, wear clock, queue bugs, RHCR A\* fix, TP port, every metric definition). Not comparable with current code; regenerate from a tagged commit before any numeric claim. `results/` is gitignored, so archive regenerated data against the commit hash. |
| Any frozen "fulfilment center" result | red | `fullfilment_center` was misspelled in `suite.rs`, `scale_tests` and `verification`; the old silent topology fallback ran those configs on `compact_grid`. Fixed in `b474475`. |

---

## Metrics

| Metric | Status | Notes |
|---|---|---|
| Fault Tolerance (FT) | yellow | `mean(P_f)/mean(P_b)` over `[t_f, T]` (a cumulative ratio). Null-perturbation floor (`null_perturb`, one agent delayed one tick): PIBT 0.994 ± 0.011, TP 0.996 ± 0.022, RHCR-PBS 0.952 ± 0.137 (one seed 0.564). Report FT against that floor; RHCR's chaotic sensitivity means single-cell RHCR differences need many seeds. Undefined (NaN) when the post-fault baseline completes nothing; gridlocked baselines are flagged (below). |
| Time-weighted Throughput Error (TWTE) | yellow | Now `Σ (t−t_f)·|1 − C_f(t)/C_b(t)| / Σ (t−t_f)`, with `C` the cumulative completions since `t_f` (`metrics.rs` `compute_twte`). Bounded, comparable across post-fault window lengths. Ticks before the baseline's first post-fault completion are counted in `twte_undefined_ticks`, not silently skipped. PIBT SD n=40: null 0.047, Zone-Out 0.092, Burst-20 0.263, Intermittent 0.275. The old unnormalised per-tick form was 85–105% noise floor and is gone. Field name stays `itae` in code/CSV/JSON. |
| Rapidity | red (per run) / yellow (recovered fraction) | Windowed ratio `R_w` (`RATIO_WINDOW`=20) evaluated only on full post-fault windows; requires an observed degradation; non-recovery is right-censored at `T − t_f` with `rapidity_recovered=false` (no silent NaN drop). **Per-run values are below the noise floor**: two independent fault-free runs "degrade" and "recover" in 90/90 pairs (median 46 ticks) at W=20, 50 and 100. Only the recovered fraction separates faults from the null, and only at W=100 (Burst 3/10 vs null 90/90). Do not claim per-run Rapidity differences. Still computed for permanent faults. |
| Critical Time (CT) | yellow | Fraction of post-fault ticks with `R_w(t) < CRITICAL_TIME_THRESHOLD` (0.5); faulted and baseline smoothed identically. Shared `src/analysis/ct.rs` used by experiment and observatory scorecard. Null floor ≈ 0.05 vs 0.09–0.17 under Zone/Burst-20/Intermittent (PIBT SD n=40). |
| Cascade Depth | yellow | Rebuilt: ADG captured from a per-fault-group snapshot taken at the fault instant, before victims are removed or plans cleared (`runner.fault_snapshots`); time-indexed type-2 edges (B depends on A when B's planned cell at step k is one A occupies at an earlier step); common lookahead `ADG_LOOKAHEAD`=1 for every solver so depth no longer measures planning horizon; multi-source BFS per fault group. Permanent faults are no longer tautologically 0 (PIBT Burst-20 1.10, RHCR/TP 0.40). Limitation: H=1 sees only the immediate blocking chain; delays that surface 2+ steps later are missed. |
| Attack Rate (AR) | yellow | `attack_rate` is now the **secondary** attack rate: among agents never directly faulted, the fraction reached by any fault cascade. No longer echoes the kill fraction (PIBT Burst-20 0.059 vs 1−survival 0.200). NaN when every agent was directly faulted — on Intermittent this happens in ~8/10 runs, so AR is uninformative there. The pre-rebuild total rate is exported as `attack_rate_total` for comparability. |
| Baseline validity (`baseline_gridlocked`) | green | Flag set when the baseline completes no task for `GRIDLOCK_WINDOW`=100 consecutive ticks after its first completion. Calibrated on 300 baselines (longest healthy gap 32 ticks). Summaries exclude flagged runs and report `baseline_gridlocked_n`. Pre-fix rates: RHCR SD n=60 20/30, RHCR SD n=40 1/30, TP SD n=40 1/30, PIBT 0. Absolute window calibrated for n ≥ 20. |
| `pbs_partial_rate` (RHCR probe) | green | Every PBS window that ends with conflicts is labelled Partial and counted (the old `to_partial_result` labelled conflicting best nodes `Solved`). Test `rhcr_unsolvable_window_counts_as_partial`. `extra_replan_count()` and `congested_window_rate()` are available on the solver but not yet exported to CSV. |
| Statistics (`stats.rs`, `stats.py`) | green | Exact Student-t quantile for every df (no 1.96 shortcut above 30); one-sample t CI (not "Welch"); Cliff's δ excludes NaN instead of counting it as a tie; Python `benjamini_hochberg` fixed to the standard BH step-up (it over-adjusted before). `fdr_pairwise.py` drops gridlocked runs, tests Rapidity's recovered fraction with Fisher's exact test and Rapidity only among recovered runs. |

---

## Fault models

| Scenario | Status | Parameter provenance | Real-world analog | Notes |
|---|---|---|---|---|
| Burst-20%, Burst-50% | yellow | Design choice | "Motor failure" analog | Kills round(n·p) agents at tick 100; victims drawn from `fault_rng` (previously the task stream). A 0% burst now kills nobody (it used to kill one). Dead robots become permanent obstacles. |
| Wear-Medium (β=2.5, η=500) | yellow | Uncited survey | "Battery degradation" analog | Clock is now **ticks alive** (was moves made, which made death timing solver-dependent); death ticks identical across solvers for a seed (`wear_deaths_happen_at_the_same_tick_under_every_solver`). F(500) = 63.2%. η in ticks; no hour→tick calibration. |
| Wear-High (β=3.5, η=150) | yellow | Carlson & Murphy 2005 (category only) | "Battery degradation" analog | F(500) = 100% (90% dead by tick 190, 99% by 232) — effectively fleet extinction; FT there tracks survival more than solver behaviour. Report survival alongside FT. (Wear-Low β=2.0, η=900: 26.6%.) |
| Zone-Out (50-tick) | yellow | Design choice | "Network partition" analog | At tick 100 the map is split into 4 vertical strips, one picked by `fault_rng`; agents inside it at that instant are frozen for 50 ticks (not a persistent zone). |
| Intermittent (MTBF=80, rec=15) | yellow | Design choice | "Sensor glitch" analog | Per-agent Exp(80) inter-arrival from tick 80, 15-tick freeze each. Memoryless; secondary AR mostly NaN (nearly every agent is hit directly). |
| `null_perturb` (control) | green | Control | — | Delays ONE agent (chosen by `fault_rng`) by one tick at tick 100. Measures the chaotic noise floor of every metric. Not yet known to the CLI TOML runner. |

---

## Solvers

| Solver | Status | Notes |
|---|---|---|
| PIBT | green | Matches pibt2 PIBT_MAPD priority semantics: priority starts at 0, order = carrying-a-task, then priority, then seeded per-agent tie-breaker; priorities stored per agent id (no global reset on death). Deviations listed in `docs/solver_fidelity.md`. |
| RHCR-PBS | yellow | Replans every W=5 ticks with a 10-tick planning window (reference README values); congestion no longer shortens the interval (counted only). **Root cause of the old dense-cell gridlock fixed**: PBS's A\* could end a path on a cell a higher-priority agent crosses later, which PBS cannot resolve; A\* now ends only on holdable cells. SD n=60 baseline 0.684 (0.658–0.698) vs 0.368 (0.066–0.660) before. Deviations: plans agents without a plan between windows (`extra_replan_count`, ~398/500 ticks), hold-endpoint goals, 3n node budget (floor 50). Yellow because SD n=20/40 and DD n=80/120 were not re-measured on the final code, and the null floor shows high chaotic sensitivity. |
| Token Passing | yellow | Ported from Ma et al. 2017 / pibt2 `tp.cpp`: own task selection (endpoint rule, nearest pickup, index order), parking at non-task endpoints, `checkAstarFin` arrival rule. Cap = parking cells per map (`max_agents(&zones)`): SD-w1 60, SD-w2 66, SD-w3 78, DD 120; other maps 0. **Only SD-w3 satisfies Ma et al.'s well-formedness condition 3** — no completeness guarantee on SD-w1, SD-w2, DD. Throughput 10–35% below the old non-faithful TP; no collapsed seeds. 12 deviations in `docs/solver_fidelity.md`. |

---

## Determinism + reproducibility

| Property | Status | Notes |
|---|---|---|
| Bit-identical replay within a machine | green | ChaCha8 RNG, no wall-clock in RNG chain |
| Paired-baseline RNG isolation | green | One stream per purpose: task demand (`rng`, consumed only by task creation), recycle and queue-join shuffles (per-tick `keyed_rng`), solver (`SOLVER_RNG_SALT`, re-keyed every tick), faults (`fault_rng`). A fault perturbs only the decisions it touches. Tests: `solver_rng_consumption_does_not_leak_into_other_streams`, `burst_equals_direct_kill_of_the_same_agents`, `zero_magnitude_faults_leave_the_run_identical` (FT exactly 1, TWTE exactly 0 for all three solvers). Post-fault trajectories still diverge chaotically — that is fault effect, quantified by `null_perturb`. |
| Cross-machine bit-identity | yellow | There is no float reduction inside a run (rayon parallelises across runs; the PBS root step is an order-preserving map). The real risk is libm `ln`/`powf` in Weibull and intermittent sampling: a 1-ULP difference that flips a rounded tick changes the whole trajectory. Run each experiment on one machine. |
| Baseline caching correctness | green | `BaselineKey` now includes `tick_count` and a hash of any custom map (test: configs differing only in tick count don't share a baseline). Cached path matches uncached bit-exactly. |
| RHCR override / ablation determinism | green | `rhcr_override_applied`, `rhcr_override_none_keeps_auto`; `BaselineKey` includes `rhcr_override_label()`. |

---

## Experiment pipeline

| Area | Status | Notes |
|---|---|---|
| `run_single_experiment` | green | Standalone paired run, tested |
| `run_matrix` + rayon parallelism | green | Parallel-safe; per-config deterministic given shared baseline key |
| Name resolution | green | Unknown topology or solver names panic in the experiment runner instead of silently substituting (the old fallback hid the `fullfilment_center` bug). The live observatory baseline (`analysis/baseline.rs`) still falls back to PIBT for an unknown solver. |
| Fleet caps | green | Matrix expansion drops solver × agent-count combos above `max_agents(&zones)` (TP: parking count). Matrix totals: E1+E2 4,320; aisle-width 4,140; Braess 3,300. |
| Collision accounting | green | `forced_waits` (moves the resolver turned into Waits) and `collision_violations` exported per run; validator checks vertex and edge-swap conflicts. Fault-free SD-w1 n=40: violations 0 for all solvers; forced waits PIBT 0, RHCR 9, TP 8. |
| Queues under faults | green | A lane is blocked only when its delivery cell or a lane cell is unwalkable (fault-free ejections 372 → 0); trips to pickups a dead robot blocks are cancelled and never re-issued; join cells count inbound agents (no two agents sent to one slot); slots follow physical lane order (TP-found deadlock fix, affects every solver). |
| CSV/JSON export | green | New per-run columns appended: `twte_undefined_ticks`, `rapidity_recovered`, `baseline_gridlocked`, `attack_rate_total`, `forced_waits`, `collision_violations`; summary adds `baseline_gridlocked_n`, `rapidity_recovered_n`, `rapidity_censored_n` (summary `rapidity_mean`/`std` renamed `rapidity_recovered_*`). Downstream Python reads by name. |
| Build isolation | green | Parallel git worktrees must NOT share `CARGO_TARGET_DIR`: cargo names workspace-member artifacts identically per worktree, so builds overwrite each other and tests can run another tree's code. |

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
