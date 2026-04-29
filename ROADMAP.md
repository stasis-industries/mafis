# MAFIS Roadmap

Known gaps, planned improvements, and non-goals.
Items are not prioritized within a section — promote to a GitHub Issue when ready to act.

---

## Recent

Recently shipped on `feat/metric-rationalization`:

- Structural cascade metric + MAX_CASCADE_DEPTH 10 → 200 (commit `2340eac`)
- Topology rename `warehouse-sd-w*` → `warehouse-single-dock-w*` (`97b65f3`); SD-w2 `number_agents` demo default capped at 72 to stay inside Token Passing's A* envelope
- Queue kick-back stranding fix: `goal = agent.pos` instead of `pickup_cell` (`2ee0313`) — resolves "agents stuck at delivery in picking state"; Loading-streak ceiling ≤ 5 ticks verified
- Token Passing goal-change sync + rewind determinism (`845a6fb`)
- Aisle-width sweep: 3 single-dock variants (w1 / w2 / w3) × 3 solvers × 6 scenarios × 30 seeds. Post-kick-back-fix re-run on 3960 priority cells (≈ 34 h 30 m wall on 8-core).

Three findings from the aisle-width sweep (post-fix, frozen):

1. **Structural cascade ∝ walkable area** at matched density — R² = 0.999 / 1.000 / 0.999 at tiers L / M / H.
2. **Mitigation Δ diverges with aisle width**, paradigm-ordered: RHCR-PBS −0.89 > PIBT −0.81 > Token Passing −0.41 per aisle step. Token Passing artefact caveat: its ADG cascade ≈ 1.01 by construction.
3. **Aggregate FT is invariant to aisle width** at matched density (PIBT 0.592 / 0.602 / 0.598 across w1 / w2 / w3) — motivates the decomposition. 6 of 150 cells flagged as Braess-paradox candidates, all RHCR-PBS at high density.

Pre-fix → post-fix drift: mean |Δ FT%| = 3.36%; comparative findings preserved.

---

## In progress

- RHCR-PBS Braess observatory probe — 3 flagged cells × 2 scenarios × 5 override corners × 20 seeds × 300 ticks. Populates the null-result correlation between `rhcr_partial_rate` and FT > 1.

---

## Planned — Solver: RHCR Improvements

### PBS — Focal Search (FOCAL_W heuristic)

**Status:** Not implemented. PBS currently uses pure-cost tie-breaking.
**Impact:** Medium — PBS throughput is below reference on dense/warehouse maps. The reference C++ implementation uses a focal list (FOCAL_W = 1.0 by default) that finds conflict-minimal solutions faster.
**What's needed:** Maintain a focal set of nodes within `[g_min, g_min * FOCAL_W]` from the open list; expand by conflict count (fewest conflicts first). Requires a separate heap sorted by `h_conflicts`, drained in parallel with the open list.
**Reference:** `src/solver/pbs_planner.rs` — `PbsPlanner::plan_window()`; reference impl at `RHCR/src/PBS.cpp` (`focal_list`).

### Travel Penalties Wired Into Windowed Planners

**Status:** `WindowContext.travel_penalties` is populated but not yet consumed by any `WindowedPlanner`.
**Impact:** Low — congestion-aware path selection is already tracked via `wait_counts` in `rhcr.rs`; the penalty layer is designed but inactive.
**What's needed:** In `priority_astar_planner.rs` and `pbs_planner.rs`, add `travel_penalties[y*w+x]` as an additive g-cost bias in `spacetime_astar_fast`/`spacetime_astar_sequential`.
**Reference:** `src/solver/windowed.rs:37` (`travel_penalties` field), `src/solver/priority_astar_planner.rs`.

### Distance Map Caching for Goal-Sequence Goals

**Status:** `fill_goal_sequences()` appends extra goals beyond the current immediate goal. The RHCR solver computes a `DistanceMap` for each agent's immediate goal but not for the extra sequence goals.
**Impact:** Low — windowed planners currently use only the first distance map for sequence goals; heuristic quality degrades for agents pursuing a chained goal.
**What's needed:** Cache `DistanceMap` per goal cell across replanning windows (evict when the goal is no longer in any agent's sequence). Pass aligned distance maps for each sequence goal into `WindowContext`.
**Reference:** `src/solver/rhcr.rs` — `rebuild_distance_maps()` and `fill_goal_sequences()`.

---

## Planned — Analysis

### Formal MTBF / MTTR Confidence Intervals

**Status:** MTBF and MTTR are point estimates.
**Impact:** Low for internal use, higher when published — reviewers will ask for CIs on these metrics.
**What's needed:** Bootstrap CI (already done for throughput via `src/analysis/metrics.rs`) applied to MTBF/MTTR time series.

### Rename `deficit_integral` / `surplus_integral` → `lost_tasks_area` / `surplus_tasks_area`

**Status:** Naming-only improvement. Code field names and CSV column headers are unchanged.
**Impact:** Low — purely a clarity improvement. The new names align with the resilience-triangle (Bruneau 2003) area framing, friendlier to reliability-engineering audiences than "integral."
**What's needed:**
- Rename `RunMetrics.deficit_integral` and `RunMetrics.surplus_integral` in `src/experiment/metrics.rs`
- Update CSV/JSON column headers in `src/experiment/export.rs`
- Update `BaselineDiff.deficit_integral` in `src/analysis/baseline.rs`
- Add a one-shot migration note so existing CSVs remain readable
**Why deferred:** Touching CSV headers invalidates downstream analysis scripts and forces re-running the dashboard. Doing this between major experiment runs avoids breaking the figure pipeline.

### Verify or replace the Critical Time threshold citation

**Status:** `CRITICAL_TIME_THRESHOLD = 0.5` in `src/constants.rs` is currently framed as an operational SLA heuristic.
**Impact:** Medium for external claims — reviewers may ask for a formal grounding of the 50% threshold.
**What's needed:** Either find a verifiable academic source (a service-level objective paper, a degraded-mode operations paper, or a specific performability text), OR commit to the operational framing and cite an industry source if such a public reference exists.

---

## Planned — Heterogeneous Robot Fleets

### Speed Tiers and Size Constraints

**Status:** All agents are identical — same speed, size, and walkability.
**Impact:** High — real warehouses mix fast pickers with slow heavy lifters. Size constraints determine which corridors a robot can enter.
**What's needed:**
- `RobotType` component: speed multiplier, footprint radius, task affinity set, Weibull β/η overrides
- Per-type `GridMap` views: wide robots mark narrow corridors as unwalkable
- Spacetime A* cost parameterized by speed (different timestep costs per agent type)
- Type-aware task assignment: only assign robot type X to zone Y

### Differentiated Wear and Battery Model

**Status:** All agents share the same Weibull wear distribution. No battery model exists.
**Impact:** Medium — heavy-load robots degrade faster; battery depletion is a distinct fault mode from mechanical wear.
**What's needed:** Per-type β/η in `FaultConfig`. New fault scenario `BatteryDepletion`: agent stops when charge reaches 0, resumes after recharge delay. Recharge stations as a new zone type.

---

## Planned — Machine Learning / Deep Reinforcement Learning

### DRL Solver (PRIMAL-style — 5th Paradigm: Learned)

**Status:** Not implemented. Current 3 solvers (PIBT, RHCR-PBS, Token Passing) use classical algorithms.
**Impact:** High research value — the key question for MAFIS is not "does DRL solve MAPF well?" but "does a learned policy degrade differently under faults than classical solvers?" DRL trained on fault-free maps may collapse faster under Weibull wear than PIBT, or generalize better due to implicit variation during training.
**What's needed:** Decentralized DRL solver where each agent acts from a local observation (k×k grid patch + goal direction + nearby agent states). Implements `LifelongSolver` trait — one `step()` call per tick, no window. Candidate architectures: PRIMAL (Sartoretti et al., 2019), MAGAT (Li et al., 2021). Would be the 4th solver alongside the current 3 classical solvers.
**Research angle:** Run the full fault scenario matrix (5 scenarios × all topologies) with the DRL solver alongside classical solvers. Compare resilience profiles in the scorecard.

### DRL Adaptive Scheduler

**Status:** Task schedulers are classical (random, closest, balanced, roundtrip). No learned scheduler exists.
**Impact:** Medium — optimal task-to-agent assignment changes mid-operation as faults accumulate. A learned policy could adapt to current fleet health and zone congestion in ways classical schedulers cannot.
**What's needed:** Contextual bandit or lightweight DRL policy that maps current state (alive agent count, per-zone congestion, fault rate, agent heat distribution) to task assignment decisions. Implements `TaskScheduler` trait — drop-in replacement.
**Research angle:** Does adaptive scheduling improve resilience compared to any fixed scheduler under progressive Weibull wear?

### ML Fault Prediction (LSTM on Heat Time Series)

**Status:** Failure ticks are pre-sampled from Weibull at init — each agent's death is predetermined. No online prediction exists.
**Impact:** Medium — proactive redeployment before failure avoids cascade effects. Enables a new fault scenario type: `PredictiveMaintenance`.
**What's needed:** LSTM or small Transformer trained on `(operational_age, heat, task_leg)` time series per agent → failure probability in next N ticks. New scenario: agent retires gracefully when predicted risk exceeds threshold, before the physical fault fires. Compare reactive vs predictive handling in the scorecard.
**Data source:** MAFIS already logs heat levels every tick — training data is free from existing simulations.

### Meta-RL Solver Selection

**Status:** Solver is fixed at simulation start. No runtime switching exists.
**Impact:** Low-medium — the optimal solver changes mid-operation (low density → PIBT fine; high density + cascading faults → RHCR better). Adaptive switching could outperform any fixed solver.
**What's needed:** Multi-armed bandit or contextual DRL that selects the active solver based on current state features (density, recent throughput delta, fault rate, alive count). No MAPF policy training needed — just a meta-policy over existing solvers. Requires `ActiveSolver` hot-swapping without resetting agent plans.
**Prerequisite:** Heterogeneous fleet support (otherwise the state space is too simple to warrant learned switching).

---

## Deferred

- Solver fidelity tests vs canonical pibt2 / RHCR C++ references (gated behind a `fidelity` feature flag). Acceptance: throughput ±3%, action-sequence Hamming ≤ 5% on first 50 ticks.
- Out-of-envelope Token Passing replanning — PIBT-fallback path inside Token Passing so TP at n > 100 stops deadlocking. Would let larger TP cells run as legitimate experiments rather than "out of envelope — skip".

---

## Non-Goals

These are intentionally out of scope for MAFIS:

- **One-shot solver support** (CBS, LaCAM, PBS one-shot, LNS2) — MAFIS measures fault resilience under sustained operation; one-shot solvers are not lifelong-capable.
- **Optimal path planning** — MAFIS is a fault observatory, not a solver benchmark. Suboptimality is acceptable if throughput and resilience are measurable.
- **Real robot integration** — simulation-only by design.
