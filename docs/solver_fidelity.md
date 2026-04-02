# Solver Fidelity Matrix

Audit date: 2026-03-30. Updated with all 7 solver sections.

## Summary

| Solver | Paper | Status | Notes |
|--------|-------|--------|-------|
| PIBT | Okumura et al. 2019 (AAAI) | Property-verified | 12 unit tests, collision-free, determinism, saturation |
| RHCR (PBS) | Li et al. 2021 (AAAI) | Property-verified | 10 unit tests, fallback tested, windowed replanning |
| RHCR (PIBT-Window) | Li et al. 2021 (AAAI) | Property-verified | 1 unit test, collision-free verified |
| RHCR (Priority A*) | Li et al. 2021 (AAAI) | Property-verified | 2 unit tests, collision-free verified |
| Token Passing | Ma et al. 2017 (AAMAS) | Property-verified | 4 unit tests, edge-swap verified |
| TPTS | Ma et al. 2017 Alg. 2 | Line-audited, 1 fix | 4 documented deviations |
| RT-LaCAM | Liang et al. 2025 (SoCS) | Line-audited, 5 fixes | 2 documented deviations remain |

**Verification methodology:**
- "Line-audited" = implementation compared against paper pseudocode, deviations documented.
- "Property-verified" = algorithmic properties from the paper are tested (saturation, collision-freedom, determinism, liveness) but no line-by-line pseudocode comparison was done.

All 7 solvers pass: collision-free verification (500 ticks), deterministic replay (all solvers x all schedulers), metamorphic properties (MR1-MR4), and rewind determinism.

---

## PIBT (Okumura et al., AAAI 2019)

**Files:** `src/solver/pibt/solver.rs`, `src/solver/shared/pibt_core.rs`

| Requirement | Status |
|-------------|--------|
| Priority inheritance (higher-priority agent inherits from blocker) | MATCH |
| One-step planning per tick | MATCH |
| Deterministic tie-breaking | MATCH (shuffle seed from tick number) |
| Grid-based 4-connected movement | MATCH |
| O(n log n) per timestep | MATCH |
| Optional cell-level guidance bias | EXTENSION (not in paper) |

**Verified properties:**
- Throughput saturation at high density (calibration test)
- Collision-free on all topologies (verification test, 500 ticks)
- Deterministic across seeds (verification test)
- Liveness: throughput > 0 for all tested configurations

**No deviations documented.** The PibtCore implementation follows the priority-inheritance backtracking algorithm. Lazy clearing of the occupation grid is a performance optimization that doesn't affect correctness.

---

## RHCR (Li et al., AAAI 2021)

**Files:** `src/solver/rhcr/solver.rs`, `src/solver/rhcr/pbs_planner.rs`, `src/solver/rhcr/pibt_planner.rs`, `src/solver/rhcr/priority_astar.rs`

| Requirement | Status |
|-------------|--------|
| Windowed replanning every W ticks | MATCH |
| Planning horizon H steps ahead | MATCH |
| Configurable inner planner | MATCH (PBS, PIBT-Window, Priority A*) |
| Goal sequence for multi-goal agents | MATCH |
| Fallback on planner failure | EXTENSION (3 modes: PerAgent, Full, Tiered) |
| Congestion detection | EXTENSION (dynamic replan shortening at >50% stuck) |
| Auto-tuning of H, W, node_limit | EXTENSION (`RhcrConfig::auto()`) |

**Three planner modes:**
1. **PBS** (`pbs_planner.rs`, 10 tests): Priority-Based Search with node limit. Faithful to Li et al. Section 4.
2. **PIBT-Window** (`rhcr/pibt_planner.rs`, 1 test): Unrolls PIBT for H steps. Uses shared PibtCore.
3. **Priority A*** (`rhcr/priority_astar.rs`, 2 tests): Sequential spacetime A* with priority ordering.

**Extensions beyond paper:**
- Three fallback modes when windowed planner fails (paper uses single PIBT fallback)
- Congestion detection shortens replan interval when >50% agents stuck
- `RhcrConfig::auto()` selects H, W, node_limit based on grid_area and num_agents

---

## Token Passing (Ma et al., AAMAS 2017)

**Files:** `src/solver/token/token_passing.rs`, `src/solver/token/common.rs`

| Requirement | Status |
|-------------|--------|
| Shared TOKEN data structure (all agents' planned paths) | MATCH |
| Sequential per-agent planning | MATCH |
| Spacetime A* against TOKEN constraints | MATCH |
| Tasked agents planned before idle agents | MATCH (PIBT_MAPD-style prioritization) |
| Constraint index from other agents' paths | MATCH (MasterConstraintIndex) |

**Verified properties:**
- No edge-swap violations (verification test)
- Deterministic replay (verification test)
- Collision-free on all topologies (verification test)

**Implementation note:** MasterConstraintIndex uses reference-counted vertex/edge constraint buffers for O(1) add/remove per agent path. This is an efficiency optimization over the naive approach (rebuild constraints from scratch) but produces identical constraint sets.

---

## TPTS (Ma et al., AAMAS 2017, Algorithm 2)

**Files:** `src/solver/token/tpts.rs`, `src/solver/token/common.rs`

| Requirement | Status |
|-------------|--------|
| Sequential token planning | MATCH |
| A* spacetime cost for swap | MATCH (fixed) |
| Snapshot before swap attempt | MATCH |
| Restore on swap failure | MATCH |

**Fix applied:** Both agents' paths are now removed from the constraint index
before the A* cost probes. Previously, each agent's own path inflated its
cost estimate, suppressing beneficial swaps.

**Documented deviations (4 total, in source header):**
1. No recursive GetTask (MAFIS TaskScheduler owns task assignment)
2. No Path2 endpoint parking (idle agents wait in place)
3. Swap cooldown to prevent oscillation (not in paper)
4. Bidirectional total-cost criterion (paper only checks one direction)

## RT-LaCAM (Liang et al., arXiv:2504.06091, SoCS 2025)

**Files:** `src/solver/rt_lacam/solver.rs`

| Requirement | Status |
|-------------|--------|
| Lazy constraint DFS | MATCH |
| PIBT as config generator | MATCH (fixed: seed diversity) |
| Rerooting | PARTIAL |
| Persistent explored map | MATCH |
| Zobrist hashing | MATCH |
| Budget-bounded expansion | MATCH (fixed: early exit removed) |
| Memory cap + restart | MATCH |
| Horizon enforcement | MATCH (fixed: stale goal cleared) |

**Fixes applied (5):**
1. Removed MAX_DEPTH1_CANDIDATES early exit. Full budget now used for DFS.
2. PIBT seed now varies per node/depth (`zobrist_seed ^ node_id ^ lln.depth`).
3. max_horizon enforced in extract_next_config. Stale goal_node cleared after reroot.
4. Removed redundant O(n^2) collision check in generate_config.
5. Stopped re-pushing existing nodes to front of open (prevents cycling).

**Documented deviations (2, in source header):**
1. Rerooting only swaps the direct parent edge, not the full chain.
   BFS fallback handles deeper re-routing.
2. Rerooted node g is set to 0 (treated as new root).
