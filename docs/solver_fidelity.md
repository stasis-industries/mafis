# Solver Fidelity Matrix

Audit date: 2026-03-28. Each solver assessed against its source paper.

## Summary

| Solver | Paper | Fidelity | Critical Issues | Action |
|--------|-------|----------|-----------------|--------|
| PIBT | Okumura 2022 (IJCAI) | ~95% | None | None needed |
| RHCR (PBS) | Li et al. 2021 (AAMAS) | ~95% | None | None needed |
| RHCR (PIBT-Window) | Li et al. 2021 (AAMAS) | ~95% | None | None needed |
| RHCR (Priority A*) | Li et al. 2021 (AAMAS) | ~95% | None | None needed |
| Token Passing | Ma et al. 2017 (AAMAS) | ~95% | None | None needed |
| PIBT+APF | Pertzovsky et al. 2025 | ~90% | 1 (idle APF) | Fix: idle agents contribute APF |
| TPTS | Ma et al. 2017 Alg. 2 | ~75% | 1 (tainted cost) | Fix: remove agent paths before cost probe |
| RT-LaCAM | Liang et al. 2025 (SoCS) | ~60% | 4 | Fix: early exit, seed, horizon, collision check |

## PIBT+APF (Pertzovsky et al., arXiv:2505.22753)

**Files:** `src/solver/apf_guidance.rs`, `src/solver/pibt_core.rs`

| Requirement | Status | Location |
|-------------|--------|----------|
| Sequential APF inside PIBT recursion | MATCH | pibt_core.rs:286-330 |
| Exponential decay w*gamma^(-dist) | MATCH | pibt_core.rs:782 |
| Goal cell returns bias 0 | MATCH | pibt_core.rs:299-301 |
| Parameters match Table 1 (w=0.1, γ=3, d_max=2, t_max=2) | MATCH | constants.rs:202-212 |
| NOT a GuidanceLayer wrapper | MATCH | apf_guidance.rs:72 (direct LifelongSolver impl) |
| Integration via cell_bias mechanism | MATCH | pibt_core.rs:605, 649-656 |

**Issues to fix:**
1. **Idle agents don't contribute APF** (pibt_core.rs:248-255). Pre-decided idle agents skip `add_apf_for_agent`. Under fault conditions (many immobile agents), tasked agents can plan through occupied cells. Fix: add APF for idle agents after pre-decision.
2. **APF projection starts from next_pos, not current_pos** (pibt_core.rs:325). Paper projects from current config. Minor impact with d_max=2. Document as deviation.
3. **Stale comment in guidance.rs:7** claims pibt+apf uses GuidedSolver. Update.

## TPTS (Ma et al., AAMAS 2017, Algorithm 2)

**Files:** `src/solver/tpts.rs`, `src/solver/token_common.rs`

| Requirement | Status | Location |
|-------------|--------|----------|
| Sequential token planning | MATCH | tpts.rs:460-502 |
| A* spacetime cost for swap | MATCH | tpts.rs:209-221 |
| Snapshot before swap attempt | MATCH | tpts.rs:333 |
| Restore on swap failure | MATCH | tpts.rs:365-368 |
| Recursive GetTask | DOCUMENTED-DEVIATION | tpts.rs:14-15 |
| Path2 endpoint parking | DOCUMENTED-DEVIATION | tpts.rs:17-19 |
| Swap cooldown (not in paper) | DOCUMENTED-DEVIATION | tpts.rs:20-23 |

**Issues to fix:**
1. **CRITICAL: Tainted cost probe** (tpts.rs:296-311). Cost comparison runs A* against master_ci that still contains BOTH agents' paths. Agent i's cost to reach j's goal is inflated by i's own path as an obstacle. Fix: remove both agents' paths from master_ci before the four cost probes, restore if swap rejected.
2. **Undocumented deviation: bidirectional total-cost criterion** (tpts.rs:309-320). Paper only checks `dist(ai, task.s) < dist(aj, task.s)`. MAFIS also requires total cost to decrease. Document as 4th deviation.
3. **Manhattan pre-filter may suppress valid swaps** (tpts.rs:283-291). In obstacle-heavy maps, Manhattan distance doesn't reflect actual A* cost. Document as performance heuristic with known trade-off.

## RT-LaCAM (Liang et al., arXiv:2504.06091, SoCS 2025)

**Files:** `src/solver/rt_lacam.rs`, `src/solver/pibt_core.rs`

| Requirement | Status | Location |
|-------------|--------|----------|
| Lazy constraint DFS | MATCH | Two-level HighLevelNode + LowLevelNode structure |
| PIBT as config generator | MATCH (with seed bug) | generate_config → one_step_constrained |
| Rerooting | PARTIAL | rt_lacam.rs:253-282 (single edge only) |
| Persistent explored map | MATCH | explored: HashMap survives across ticks |
| Zobrist hashing | MATCH | Formula-based, O(n), agent-index-sensitive |
| Budget-bounded expansion | MATCH | expanded < node_budget |
| Memory cap + restart | MATCH | arena.len() > MAX_VISITED → restart |

**Issues to fix:**
1. **CRITICAL: MAX_DEPTH1_CANDIDATES early exit** (rt_lacam.rs:378). Stops expansion after 5 depth-1 children, ignoring remaining budget. Degrades to greedy-PIBT. Fix: remove the counter and early return; let budget be sole termination.
2. **CRITICAL: Fixed PIBT seed** (rt_lacam.rs:222). `set_shuffle_seed(self.zobrist_seed)` makes all config generations produce identical PIBT output. Fix: use `zobrist_seed ^ node_id ^ lln.depth` for diversity.
3. **CRITICAL: max_horizon never enforced** (rt_lacam.rs:106). Computed but unused. extract_next_config can backtrack across arbitrarily deep stale nodes. Fix: reject goal_node when g exceeds max_horizon from current_node.
4. **CRITICAL: O(n²) collision check in generate_config** (rt_lacam.rs:232-238). PIBT already guarantees vertex-conflict freedom. Remove the redundant quadratic loop.
5. **Partial reroot** (rt_lacam.rs:273). Only swaps one parent edge; deeper ancestors keep stale pointers. Falls through to BFS fallback every tick.
6. **Stale goal_node** (rt_lacam.rs:393). Never cleared after reroot. Fix: check `goal_node.g <= current_node.g` and clear.
7. **Re-push existing nodes to front of open** (rt_lacam.rs:454). Can cause cycling. Paper doesn't re-push.
8. **Rerooted node g=0 hardcode** (rt_lacam.rs:260). Latent bug after second reroot on revisited config.
