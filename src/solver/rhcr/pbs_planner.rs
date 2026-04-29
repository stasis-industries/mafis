//! PBS (Priority-Based Search) planner for RHCR's windowed planning.
//!
//! Builds a binary priority tree: each node has a priority ordering. When a
//! conflict is found, the tree branches into two children (agent_i > agent_j
//! and agent_j > agent_i). Bounded by node limit.
//!
//! REFERENCE: docs/papers_codes/rhcr/src/PBS.cpp (Jiaoyang-Li/RHCR, AAAI 2021).
//! Paper: Li, Tinka, Kiesel, Durham, Kumar, Koenig — "Lifelong Multi-Agent Path
//! Finding in Large-Scale Warehouses".
//!
//! This implementation ports the canonical reference's **eager-priority PBS**
//! (the reference's default `lazyPriority = false`, driver.cpp:114) with the
//! following features:
//!
//! 1. **Eager priority resolution via `find_consistent_paths`** (PBS.cpp:410) —
//!    when a new priority pair (higher → lower) is added in `try_branch`, every
//!    agent whose path is invalidated by the new constraint is iteratively
//!    replanned until fixed-point. Implemented by `find_consistent_paths` here.
//!    Lazy mode is still available via `PbsPlanner::new_lazy()` for regression
//!    benchmarking.
//!
//! 2. **`prioritize_start` heuristic** (PBS.cpp:385) — when a conflict arises,
//!    check whether either agent is still waiting at its start cell at the
//!    conflict timestep. If so, only the *other* agent (the one that has
//!    actually moved) is added to the replan set. Implemented by `wait_at_start`
//!    + `find_replan_agents`.
//!
//! 3. **`choose_conflict` with nogood override** (PBS.cpp:216-293) — the default
//!    conflict selection is the earliest-time conflict (matches MAFIS's prior
//!    behavior), but if any conflict pair `(a, b)` is in the `nogood` set it
//!    is returned immediately to surface unresolvable pairs early and prune
//!    faster.
//!
//! 4. **`nogood` set** (PBS.cpp:772) — when both children of a branch fail,
//!    the pair `(min(a, b), max(a, b))` is inserted into `nogood` so the next
//!    time that pair appears in a conflict, `choose_conflict` picks it
//!    immediately. Avoids re-walking dead subtrees.
//!
//! 5. **Sum-of-costs (`g_val`) tie-break** (PBS.cpp:630, 750) — best-node
//!    update uses `(earliest_collision DESC, g_val ASC)`. Child push order
//!    uses `(g_val ASC, num_collisions ASC)` so the better child is popped
//!    first by DFS. `g_val` is recomputed by summing `plans[i].len()` across
//!    all agents on every node creation (full recompute — trivial relative to
//!    A* cost and avoids incremental bookkeeping bugs).
//!
//! 6. **Horizon-bounded best-effort sequential A*** — the reference's
//!    `StateTimeAStar::run` returns a best-effort partial path when the time
//!    budget is exhausted before reaching all goals, rather than failing. The
//!    MAFIS port matches this in `spacetime_astar_sequential`: on open-set
//!    exhaustion, the function tracks `(goal_id DESC, heuristic ASC, time ASC)`
//!    and returns the path to the "closest-to-next-goal" node it popped. This
//!    is what unlocks RHCR-PBS from ~0.020 to ~0.45 tasks/tick on
//!    warehouse_single_dock — without it, `plan_agent` fails for any agent whose
//!    primary goal is farther than `horizon` and PBS falls back to single-step
//!    PIBT. See `src/solver/shared/astar.rs` for the goal-sequence A* details.
//!
//! ## Deliberate deviations from the reference
//!
//! - **Deterministic node budget** instead of the reference's 60-second time
//!   budget (PBS.cpp:675-681). Time budgets break determinism across hardware,
//!   which would invalidate MAFIS's rewind/replay guarantees. The budget is
//!   `PBS_MAX_NODE_LIMIT` (1,000 on wasm / 10,000 on native). Accepted in
//!   exchange for reproducibility.
//!
//! ## Historical deviations now closed
//!
//! Before the RHCR-PBS fidelity port, MAFIS's RHCR-PBS deviated from the reference in
//! five ways: (1) lazy priority resolution only, (2) no `prioritize_start`,
//! (3) no `nogood` set, (4) `conflicts ASC` tie-break instead of `g_val ASC`,
//! (5) single-goal A* without goal sequences. The combination of (1) and (5)
//! caused `plan_agent` to fail for any agent whose pickup-delivery distance
//! exceeded `horizon = 15`, so PBS exhausted after 2 nodes on warehouse_single_dock
//! and the wrapper fell back to per-agent PIBT — pinning RHCR-PBS throughput
//! to ~0.020 tasks/tick, 32-40× below the published reference. All five
//! deviations are now closed.

use bevy::prelude::*;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Reference-counted per-agent plan slot. The PBS tree shares inner `Vec`s
/// across sibling nodes and the best-node snapshot, avoiding O(n) deep-clones
/// per branch. Only the replanned agent's slot is replaced (`Arc::new(...)`)
/// inside `find_consistent_paths`; all other siblings keep pointing at the
/// parent's `Vec`. See Step 7 of the 2026-04-08 RHCR-PBS perf sprint.
type ArcPlan = Arc<Vec<Action>>;
/// Reference-counted per-agent timeline slot. Same copy-on-write semantics
/// as `ArcPlan`. Rebuilt inside `rebuild_timeline` whenever an agent's plan
/// changes.
type ArcTimeline = Arc<Vec<IVec2>>;

use crate::constants::{PBS_CONSISTENT_PATHS_REPLAN_MULT, PBS_PRIORITIZE_START_DEFAULT};
use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::windowed::{PlanFragment, WindowAgent, WindowContext, WindowResult, WindowedPlanner};
use crate::solver::shared::astar::{
    FlatCAT, FlatConstraintIndex, SeqGoalGrid, spacetime_astar_sequential,
};
use crate::solver::shared::heuristics::{DistanceMap, DistanceMapCache};

// ---------------------------------------------------------------------------
// PBS Node
// ---------------------------------------------------------------------------

/// Conflict descriptor used by `find_consistent_paths`, `find_replan_agents`,
/// and PbsNode's persistent conflict list. Carries the two colliding agents
/// and the conflict timestep (needed by `wait_at_start`).
#[derive(Debug, Clone, Copy)]
struct ReplanConflict {
    a1: usize,
    a2: usize,
    t: usize,
}

/// A single node in the PBS priority tree.
struct PbsNode {
    /// Plans for each agent (index aligned with WindowContext.agents).
    /// `Arc`-wrapped per-slot so sibling nodes and the best-node snapshot
    /// share inner `Vec`s; only replanned agents get a fresh `Arc::new(...)`.
    plans: Vec<ArcPlan>,
    /// Pre-built position timelines (avoids rebuilding for conflict detection).
    /// Same copy-on-write semantics as `plans`.
    timelines: Vec<ArcTimeline>,
    /// Priority ordering constraints: (higher, lower) — `higher` plans first.
    priority_pairs: Vec<(usize, usize)>,
    /// Number of conflicts in this node (for best-first ordering).
    conflicts: usize,
    /// Earliest timestep at which a collision occurs (usize::MAX if none).
    earliest_collision: usize,
    /// Full list of active conflicts in this node. Mirrors the reference's
    /// `PBSNode::conflicts: list<Conflict>` (PBS.cpp, PBSNode struct). Used
    /// by eager mode's `find_consistent_paths` to seed the replan cascade:
    /// the reference (`generate_child`, PBS.cpp:512) copies the parent's
    /// full conflict list into the child before the cascade, so agents
    /// whose existing conflicts imply they should yield to a higher-priority
    /// agent get replanned even if they aren't directly in conflict with
    /// the newly-constrained agent.
    ///
    /// Populated lazily — in lazy mode this may be empty since the cascade
    /// is never run and the per-node conflict COUNT (`conflicts` above) is
    /// enough for best-node tracking.
    conflict_list: Vec<ReplanConflict>,
    /// Sum-of-costs across all agent plans: `sum(plans[i].len())`. Reference
    /// calls this `g_val` (PBS.cpp:524) — here we use the bare path length as
    /// the cost since RHCR's windowed planner uses unit-time travel and
    /// discards per-cell travel-time weights.
    ///
    /// Always fully recomputed from `plans` on each node creation — the
    /// reference's incremental `g_val - old + new` updates are correct but
    /// error-prone; a full `n`-addition sum is totally negligible vs the
    /// BFS-heavy A* path planning.
    g_val: u64,
    /// Node ID for tie-breaking (lower = earlier).
    id: usize,
}

/// Full sum-of-costs for a plan set.
#[inline]
fn sum_g_val(plans: &[ArcPlan]) -> u64 {
    plans.iter().map(|p| p.len() as u64).sum()
}

// ---------------------------------------------------------------------------
// Spatial conflict detection — O(n × H) replacing O(n² × H)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Conflict {
    agent_a: usize,
    agent_b: usize,
    #[allow(dead_code)]
    time: u64,
}

const NO_AGENT: u32 = u32::MAX;

/// Grid-indexed conflict detection: O(n × H) instead of O(n² × H).
struct ConflictGrid {
    /// Grid for current timestep: grid[pos_flat] = agent index
    cur: Vec<u32>,
    /// Grid for previous timestep positions
    prev: Vec<u32>,
    cells: usize,
}

impl ConflictGrid {
    fn new() -> Self {
        Self { cur: Vec::new(), prev: Vec::new(), cells: 0 }
    }

    fn ensure_size(&mut self, cells: usize) {
        if self.cells != cells {
            self.cells = cells;
            self.cur = vec![NO_AGENT; cells];
            self.prev = vec![NO_AGENT; cells];
        }
    }

    fn detect_first(
        &mut self,
        timelines: &[ArcTimeline],
        grid_w: i32,
        window: usize,
    ) -> Option<Conflict> {
        let n = timelines.len();
        if n < 2 {
            return None;
        }

        let max_t = timelines.iter().map(|tl| tl.len().saturating_sub(1)).max().unwrap_or(0);
        let max_t = max_t.min(window);

        // Fill t=0 positions into prev
        self.prev.fill(NO_AGENT);
        for (i, tl) in timelines.iter().enumerate() {
            let pos = pos_at(tl, 0);
            let flat = (pos.y * grid_w + pos.x) as usize;
            if flat < self.cells {
                self.prev[flat] = i as u32;
            }
        }

        for t in 1..=max_t {
            self.cur.fill(NO_AGENT);

            for i in 0..n {
                let pos = pos_at(&timelines[i], t);
                let flat = (pos.y * grid_w + pos.x) as usize;
                if flat >= self.cells {
                    continue;
                }

                // Vertex conflict: another agent already at this cell at time t
                if self.cur[flat] != NO_AGENT {
                    let j = self.cur[flat] as usize;
                    return Some(Conflict { agent_a: j, agent_b: i, time: t as u64 });
                }
                self.cur[flat] = i as u32;

                // Edge conflict: check if agent i swapped with someone
                let prev_pos = pos_at(&timelines[i], t - 1);
                if prev_pos != pos {
                    // Who was at my current position (pos) at t-1?
                    let pos_flat = (pos.y * grid_w + pos.x) as usize;
                    if pos_flat < self.cells {
                        let j = self.prev[pos_flat];
                        if j != NO_AGENT && j != i as u32 {
                            let j = j as usize;
                            // Agent j was at pos at t-1. If j is now at prev_pos, it's a swap.
                            let j_cur = pos_at(&timelines[j], t);
                            if j_cur == prev_pos {
                                return Some(Conflict { agent_a: i, agent_b: j, time: t as u64 });
                            }
                        }
                    }
                }
            }

            // Swap cur → prev for next iteration
            std::mem::swap(&mut self.cur, &mut self.prev);
        }

        None
    }

    /// Variant of `detect_first` that takes a nogood set and returns the first
    /// conflict whose agent pair matches any entry in `nogood` (regardless of
    /// timestep). If no nogood match is found, returns the earliest conflict
    /// (same as `detect_first`). Port of the `choose_conflict` nogood override
    /// at PBS.cpp:274-290 combined with the earliest-first fallback at
    /// PBS.cpp:247-260.
    ///
    /// `nogood` entries are pairs `(min, max)` of agent indices — we normalize
    /// on lookup so the caller does not need to worry about (a,b) vs (b,a).
    ///
    /// **Single-sweep implementation** (RHCR-PBS perf sprint 2026-04-09): the
    /// previous version did two full timeline sweeps — one via `detect_first`
    /// for the earliest conflict, then a second for the nogood scan. This one
    /// walks the timelines once, tracking the earliest conflict and returning
    /// immediately on a nogood match. When `nogood` is empty the inner loop
    /// returns on the first conflict encountered (identical semantics to
    /// `detect_first` in that case). Saves one full `prev.fill` + cells×N×H
    /// scan per PBS node when nogood has entries (common in mature trees).
    fn detect_with_nogood(
        &mut self,
        timelines: &[ArcTimeline],
        grid_w: i32,
        window: usize,
        nogood: &BTreeSet<(usize, usize)>,
    ) -> Option<Conflict> {
        let n = timelines.len();
        if n < 2 {
            return None;
        }
        let max_t = timelines.iter().map(|tl| tl.len().saturating_sub(1)).max().unwrap_or(0);
        let max_t = max_t.min(window);
        let nogood_active = !nogood.is_empty();

        // Fill t=0 positions into prev
        self.prev.fill(NO_AGENT);
        for (i, tl) in timelines.iter().enumerate() {
            let pos = pos_at(tl, 0);
            let flat = (pos.y * grid_w + pos.x) as usize;
            if flat < self.cells {
                self.prev[flat] = i as u32;
            }
        }

        // Tracks the earliest conflict seen so far. Used only when
        // `nogood_active` — when not active, the first conflict is returned
        // immediately and `earliest` never gets set.
        let mut earliest: Option<Conflict> = None;

        for t in 1..=max_t {
            self.cur.fill(NO_AGENT);

            for i in 0..n {
                let pos = pos_at(&timelines[i], t);
                let flat = (pos.y * grid_w + pos.x) as usize;
                if flat >= self.cells {
                    continue;
                }

                // Vertex conflict: another agent already at this cell at time t
                if self.cur[flat] != NO_AGENT {
                    let j = self.cur[flat] as usize;
                    let c = Conflict { agent_a: j, agent_b: i, time: t as u64 };
                    if !nogood_active {
                        return Some(c);
                    }
                    let pair = (j.min(i), j.max(i));
                    if nogood.contains(&pair) {
                        return Some(c);
                    }
                    if earliest.is_none() {
                        earliest = Some(c);
                    }
                }
                // In nogood-active mode we update `cur[flat] = i` even after a
                // non-matching conflict, matching the original second-sweep
                // behavior (PBS.cpp conflict-scan semantics). In non-active
                // mode we have already returned above on the first conflict.
                self.cur[flat] = i as u32;

                // Edge conflict: check if agent i swapped with someone
                let prev_pos = pos_at(&timelines[i], t - 1);
                if prev_pos != pos {
                    let pos_flat = (pos.y * grid_w + pos.x) as usize;
                    if pos_flat < self.cells {
                        let j = self.prev[pos_flat];
                        if j != NO_AGENT && j != i as u32 {
                            let j = j as usize;
                            let j_cur = pos_at(&timelines[j], t);
                            if j_cur == prev_pos {
                                let c = Conflict { agent_a: i, agent_b: j, time: t as u64 };
                                if !nogood_active {
                                    return Some(c);
                                }
                                let pair = (j.min(i), j.max(i));
                                if nogood.contains(&pair) {
                                    return Some(c);
                                }
                                if earliest.is_none() {
                                    earliest = Some(c);
                                }
                            }
                        }
                    }
                }
            }

            std::mem::swap(&mut self.cur, &mut self.prev);
        }

        earliest
    }

    fn count_conflicts(&mut self, timelines: &[ArcTimeline], grid_w: i32, window: usize) -> usize {
        let n = timelines.len();
        if n < 2 {
            return 0;
        }

        let max_t = timelines.iter().map(|tl| tl.len().saturating_sub(1)).max().unwrap_or(0);
        let max_t = max_t.min(window);
        let mut count = 0;

        // Fill t=0 positions into prev
        self.prev.fill(NO_AGENT);
        for (i, tl) in timelines.iter().enumerate() {
            let pos = pos_at(tl, 0);
            let flat = (pos.y * grid_w + pos.x) as usize;
            if flat < self.cells {
                self.prev[flat] = i as u32;
            }
        }

        for t in 1..=max_t {
            self.cur.fill(NO_AGENT);

            for i in 0..n {
                let pos = pos_at(&timelines[i], t);
                let flat = (pos.y * grid_w + pos.x) as usize;
                if flat >= self.cells {
                    continue;
                }

                // Vertex conflict
                if self.cur[flat] != NO_AGENT {
                    count += 1;
                }
                self.cur[flat] = i as u32;

                // Edge conflict (swap)
                let prev_pos = pos_at(&timelines[i], t - 1);
                if prev_pos != pos {
                    let pos_flat2 = (pos.y * grid_w + pos.x) as usize;
                    if pos_flat2 < self.cells {
                        let j = self.prev[pos_flat2];
                        if j != NO_AGENT && j != i as u32 {
                            let j_cur = pos_at(&timelines[j as usize], t);
                            if j_cur == prev_pos {
                                count += 1;
                            }
                        }
                    }
                }
            }

            std::mem::swap(&mut self.cur, &mut self.prev);
        }

        count
    }
}

/// Build position timelines from plans + starting positions.
/// Each timeline is fresh-allocated and wrapped in an `Arc` so PBS nodes can
/// share it across branches via pointer copies.
fn build_timelines(plans: &[ArcPlan], agents: &[WindowAgent]) -> Vec<ArcTimeline> {
    plans
        .iter()
        .zip(agents.iter())
        .map(|(plan, agent)| {
            let mut pos = agent.pos;
            let mut tl = Vec::with_capacity(plan.len() + 1);
            tl.push(pos);
            for &a in plan.iter() {
                pos = a.apply(pos);
                tl.push(pos);
            }
            Arc::new(tl)
        })
        .collect()
}

/// Rebuild a single agent's timeline — replaces the `Arc` slot with a fresh
/// `Arc::new(...)`. Sibling slots remain shared with the parent.
fn rebuild_timeline(
    timelines: &mut [ArcTimeline],
    plans: &[ArcPlan],
    agents: &[WindowAgent],
    idx: usize,
) {
    let plan = &plans[idx];
    let mut tl = Vec::with_capacity(plan.len() + 1);
    let mut pos = agents[idx].pos;
    tl.push(pos);
    for &a in plan.iter() {
        pos = a.apply(pos);
        tl.push(pos);
    }
    timelines[idx] = Arc::new(tl);
}

#[inline]
fn pos_at(timeline: &[IVec2], t: usize) -> IVec2 {
    if t < timeline.len() { timeline[t] } else { *timeline.last().unwrap() }
}

// ---------------------------------------------------------------------------
// Plan one agent with priority constraints (sequential-goal A*)
// ---------------------------------------------------------------------------

/// Plan a single agent's path under PBS priority constraints, using
/// `spacetime_astar_sequential` over the agent's goal sequence
/// (`primary_goal` followed by the peek-chain extensions in
/// `agent.goal_sequence`).
///
/// The sequential A* call attempts to satisfy all goals in order. When the
/// horizon is too short for the full chain it returns `Err(NoSolution)` and
/// we progressively pop trailing goals and retry — that gives the
/// "horizon-bounded best-effort" semantic the reference's
/// `getPathBySpaceTimeAstar` provides via the canonical `getPath` family.
///
/// **Borrow contract**: `dist_cache` is passed by `&` (immutable) so
/// references returned by `cache.get_cached(goal)` can coexist with the `&mut`
/// borrows on `seq_stg` and `ci_buf`. The caller (`plan_window`) MUST have
/// already pre-populated the cache with every goal cell (primary + chain) for
/// every agent that this function may be called on via
/// `dist_cache.get_or_compute(...)`; otherwise `cache.get_cached` returns
/// `None` and `plan_agent` returns `None` for that branch.
///
/// `goals_after_trim` is an out parameter incremented to the **number of
/// goals retained** in the call that succeeded (or the final attempted size
/// if all failed). Used by tests to verify the trim loop isn't unnecessarily
/// degrading to single-goal in the easy case.
fn plan_agent(
    agent_idx: usize,
    agents: &[WindowAgent],
    all_plans: &[ArcPlan],
    priority_pairs: &[(usize, usize)],
    grid: &GridMap,
    horizon: usize,
    ci_buf: &mut FlatConstraintIndex,
    seq_stg: &mut SeqGoalGrid,
    start_constraints: &[(IVec2, u64)],
    dist_cache: &DistanceMapCache,
    goals_after_trim: &mut usize,
) -> Option<Vec<Action>> {
    let agent = &agents[agent_idx];

    // ── Build the per-agent flat constraint index ────────────────────────
    ci_buf.reset(grid.width, grid.height, horizon as u64);

    // Start constraints (other agents at t=0). The fix-window prevents an
    // agent from planning to be at another agent's pre-window cell.
    for (j, &(pos, time)) in start_constraints.iter().enumerate() {
        if j != agent_idx {
            ci_buf.add_vertex(pos, time);
        }
    }

    // Higher-priority agents' plans become hard constraints for this agent.
    for &(higher, lower) in priority_pairs {
        if lower == agent_idx {
            let plan = &all_plans[higher];
            let higher_agent = &agents[higher];
            let mut pos = higher_agent.pos;
            for (t, &action) in plan.iter().enumerate() {
                let next_pos = action.apply(pos);
                ci_buf.add_vertex(next_pos, (t + 1) as u64);
                ci_buf.add_edge(next_pos, pos, t as u64);
                pos = next_pos;
            }
            // After plan ends, the higher agent holds its final position.
            let final_t = plan.len();
            for t in final_t..(horizon + 1) {
                ci_buf.add_vertex(pos, t as u64);
            }
        }
    }

    // ── Build the goal vector: primary + peek-chain extensions ───────────
    //
    // Each goal is paired with its DistanceMap (the per-layer admissible
    // heuristic for `spacetime_astar_sequential`). Cache references are
    // collected first, into a temporary owned `Vec<(IVec2, &DistanceMap)>`,
    // because `dist_cache` is borrowed `&` here while `seq_stg` will be
    // borrowed `&mut` by the A* call below — both borrows are simultaneously
    // live but compatible because they touch disjoint fields.
    let mut goals: Vec<(IVec2, &DistanceMap)> = Vec::with_capacity(1 + agent.goal_sequence.len());

    // Primary goal — must be in cache (plan_window pre-populates via
    // dist_cache.get_or_compute on the augmented goal set).
    if let Some(dm) = dist_cache.get_cached(agent.goal) {
        goals.push((agent.goal, dm));
    } else {
        // Cache miss on the primary goal is unexpected (plan_window should
        // always populate it). Bail rather than silently degrade — the
        // caller's PBS branch will return None and try the other branch.
        *goals_after_trim = 0;
        return None;
    }

    // Peek-chain extensions. Cache misses here are silently dropped (the
    // chain is best-effort anyway).
    for &cell in &agent.goal_sequence {
        if let Some(dm) = dist_cache.get_cached(cell) {
            goals.push((cell, dm));
        }
    }

    // ── Try the full sequence first; on failure, progressively pop ──────
    // trailing goals. This gives "horizon-bounded best-effort" semantics:
    // even when the final goal in the chain is not reachable inside `horizon`
    // ticks, the search makes meaningful progress toward the nearest
    // reachable goal in the sequence.
    // Per-call expansion budget for PBS's sequential A*. This is tighter than
    // the global `ASTAR_MAX_EXPANSIONS` (5000) because `plan_agent` is invoked
    // O(n × nodes) times per PBS window — on warehouse_single_dock/N=40 that's ~500+
    // calls per window, so a 5000-budget balloons to ~2.5M expansions/window.
    // 2000 keeps total ~1M which the best-partial fallback still handles well.
    const PBS_ASTAR_BUDGET: u64 = 2_000;
    while !goals.is_empty() {
        let result = spacetime_astar_sequential(
            grid,
            agent.pos,
            &goals,
            ci_buf,
            horizon as u64,
            seq_stg,
            PBS_ASTAR_BUDGET,
        );
        if let Ok(plan) = result {
            *goals_after_trim = goals.len();
            return Some(plan);
        }
        if goals.len() == 1 {
            // Single-goal also failed — this is the "true infeasible" case
            // (no walkable path under the constraints + horizon).
            *goals_after_trim = 1;
            return None;
        }
        goals.pop();
    }
    *goals_after_trim = 0;
    None
}

/// Root-only variant of `plan_agent` that assumes `ci_buf` has already been
/// reset and populated with the window's `start_constraints` by the caller.
/// Does NOT touch `ci_buf` — every root-node A* call reads an identical
/// constraint state, so building it once amortizes the reset + N start-vertex
/// inserts across the N root calls.
///
/// Safe use requires `priority_pairs.is_empty()` — this is the root node in
/// PBS's DFS. The cascade path (`find_consistent_paths`) must keep using the
/// full `plan_agent`, which rebuilds `ci_buf` per call to add the priority
/// pair's path constraints on top of `start_constraints`.
///
/// Note: the caller pre-fills `ci_buf` with **all** agents' start positions,
/// including the calling agent's own `(start, 0)`. This is semantically
/// equivalent to the old per-agent skip because `spacetime_astar_sequential`
/// never queries `is_vertex_blocked(start, 0)` — constraint checks only fire
/// on `next_time = current.time + 1` (see `astar.rs` A* expansion) and on
/// `cur.time` for edge constraints originating at the start, none of which
/// touch the `(start, 0)` vertex slot. Confirmed by the existing A* tests.
fn plan_agent_root_no_reset(
    agent_idx: usize,
    agents: &[WindowAgent],
    grid: &GridMap,
    horizon: usize,
    ci_buf: &FlatConstraintIndex,
    seq_stg: &mut SeqGoalGrid,
    dist_cache: &DistanceMapCache,
    goals_after_trim: &mut usize,
) -> Option<Vec<Action>> {
    let agent = &agents[agent_idx];

    // ── Build the goal vector: primary + peek-chain extensions ───────────
    let mut goals: Vec<(IVec2, &DistanceMap)> = Vec::with_capacity(1 + agent.goal_sequence.len());
    if let Some(dm) = dist_cache.get_cached(agent.goal) {
        goals.push((agent.goal, dm));
    } else {
        *goals_after_trim = 0;
        return None;
    }
    for &cell in &agent.goal_sequence {
        if let Some(dm) = dist_cache.get_cached(cell) {
            goals.push((cell, dm));
        }
    }

    // ── Trim loop: identical to `plan_agent` ─────────────────────────────
    const PBS_ASTAR_BUDGET: u64 = 2_000;
    while !goals.is_empty() {
        let result = spacetime_astar_sequential(
            grid,
            agent.pos,
            &goals,
            ci_buf,
            horizon as u64,
            seq_stg,
            PBS_ASTAR_BUDGET,
        );
        if let Ok(plan) = result {
            *goals_after_trim = goals.len();
            return Some(plan);
        }
        if goals.len() == 1 {
            *goals_after_trim = 1;
            return None;
        }
        goals.pop();
    }
    *goals_after_trim = 0;
    None
}

// ---------------------------------------------------------------------------
// prioritize_start / find_replan_agents / find_consistent_paths helpers
// ---------------------------------------------------------------------------

/// Returns true iff the agent has not moved away from its start cell by
/// time `t`. Direct port of `PBS.cpp:361-371` (`wait_at_start`).
///
/// Reference semantics: iterate the path; return `true` the instant the
/// iterator hits a state with `state.timestep > timestep` (i.e. we've walked
/// past the query timestep without ever finding a move); return `false` if
/// any intermediate state is at a location other than the start; return
/// `false` on an empty path or on a path that exhausts without the timestep
/// being exceeded.
///
/// MAFIS represents paths as action sequences, where `path[i]` is the
/// action taken between timestep `i` and `i+1`. So the position at time `k`
/// is `start` folded by `path[0..k]`. "The path covers up to time
/// `path.len()`" — if `t < path.len()`, we check positions 1..=t are all
/// equal to start. If `t >= path.len()`, the reference's "state.timestep
/// exceeded" branch never triggers and the loop exhausts → return false.
fn wait_at_start(path: &[Action], start: IVec2, t: usize) -> bool {
    if path.is_empty() {
        // Reference returns `false` on empty path.
        return false;
    }
    // If the path doesn't cover up to `t`, the reference exhausts the loop
    // and returns false.
    if t >= path.len() {
        return false;
    }
    // Check positions 1..=t (the position at t=0 is always `start`).
    let mut pos = start;
    for &a in &path[..=t] {
        pos = a.apply(pos);
        if pos != start {
            return false;
        }
    }
    true
}

/// Returns `true` iff agent `x` is transitively above agent `y` in the
/// priority DAG (i.e. some `x -> ... -> y` path exists in the `(higher,
/// lower)` edge set). Equivalent to `priorities.connected(x, y)` in the
/// reference (see `PriorityGraph::connected`).
///
/// Pooled-buffer variant: caller owns `stack` and `seen` (`Vec<bool>` indexed
/// by agent id, which is much cheaper than a `HashSet<usize>` at n ≤ 40).
/// `stack` is cleared on entry; `seen` is cleared and resized to the highest
/// agent index referenced.
fn priority_connected_buf(
    stack: &mut Vec<usize>,
    seen: &mut Vec<bool>,
    pairs: &[(usize, usize)],
    x: usize,
    y: usize,
) -> bool {
    if x == y {
        return false;
    }
    let mut max_idx = x.max(y);
    for &(h, l) in pairs {
        if h > max_idx {
            max_idx = h;
        }
        if l > max_idx {
            max_idx = l;
        }
    }
    stack.clear();
    seen.clear();
    seen.resize(max_idx + 1, false);
    stack.push(x);
    seen[x] = true;
    while let Some(node) = stack.pop() {
        for &(h, l) in pairs {
            if h == node {
                if l == y {
                    return true;
                }
                if !seen[l] {
                    seen[l] = true;
                    stack.push(l);
                }
            }
        }
    }
    false
}

/// Port of `PBS.cpp:374-407` (`find_replan_agents`). Given the current node's
/// priority pairs and current plans, and a list of `new_conflicts` introduced
/// by the most recent replan, extend `replan` with the set of agents that
/// must be replanned.
///
/// Semantics per the reference:
/// * If either agent is already in `replan`, skip the conflict.
/// * Else if `prioritize_start && wait_at_start(a1)`, add `a2` (the moving
///   agent) and skip. (Replanning the moving agent is more constructive
///   than replanning an agent that hasn't started.)
/// * Else if `prioritize_start && wait_at_start(a2)`, add `a1`.
/// * Else if `a1` is transitively below `a2` in the priority DAG, add `a1`
///   (its plan is stale because `a2`'s constraints should dominate).
/// * Else if `a2` is transitively below `a1`, add `a2`.
/// * Else the conflict pair isn't constrained by the current priorities and
///   nothing is inserted (the branching step will add a new constraint).
///
/// Direction note: MAFIS stores priority edges as `(higher, lower)`, so
/// "a1 is transitively below a2" in the priority DAG means "there is a path
/// from a2 to a1 in the (higher, lower) edge set" — hence the argument
/// order in the `priority_connected` call below. The reference's
/// `PriorityGraph` stores edges in the reverse direction (lower → higher),
/// so its `connected(a1, a2)` returns true under the same condition.
#[allow(clippy::too_many_arguments)]
fn find_replan_agents(
    priority_pairs: &[(usize, usize)],
    plans: &[ArcPlan],
    agent_starts: &[IVec2],
    new_conflicts: &[ReplanConflict],
    prioritize_start: bool,
    replan: &mut BTreeSet<usize>,
    pc_stack: &mut Vec<usize>,
    pc_seen: &mut Vec<bool>,
) {
    for c in new_conflicts {
        let a1 = c.a1;
        let a2 = c.a2;
        if replan.contains(&a1) || replan.contains(&a2) {
            continue;
        }
        let t = c.t;
        if prioritize_start && wait_at_start(&plans[a1], agent_starts[a1], t) {
            replan.insert(a2);
            continue;
        }
        if prioritize_start && wait_at_start(&plans[a2], agent_starts[a2], t) {
            replan.insert(a1);
            continue;
        }
        // "a1 is transitively below a2" ⇔ MAFIS `(higher, lower)` edge set
        // has a path a2 → a1. Test: reverse the direction to measure
        // sensitivity.
        if priority_connected_buf(pc_stack, pc_seen, priority_pairs, a1, a2) {
            replan.insert(a1);
            continue;
        }
        if priority_connected_buf(pc_stack, pc_seen, priority_pairs, a2, a1) {
            replan.insert(a2);
            continue;
        }
    }
}

/// Detect every pairwise conflict on the current timelines and materialize
/// them into an owned list. Used by `find_consistent_paths` to seed the
/// cascade's initial replan set from the parent node's conflicts.
///
/// This is O(n² × H) — acceptable for PBS where `n` is the per-window
/// agent count (typically ≤ 40 in the regression tests) and `H` is the
/// planning horizon (~10-20).
fn all_pairwise_conflicts(timelines: &[ArcTimeline], window: usize) -> Vec<ReplanConflict> {
    let n = timelines.len();
    let mut out: Vec<ReplanConflict> = Vec::new();
    if n < 2 {
        return out;
    }

    for a in 0..n {
        for b in (a + 1)..n {
            let tl_a = &timelines[a];
            let tl_b = &timelines[b];
            let max_a = tl_a.len().saturating_sub(1);
            let max_b = tl_b.len().saturating_sub(1);
            let max_t = max_a.max(max_b).min(window);

            let mut found = false;
            // Vertex conflict
            for t in 0..=max_t {
                if pos_at(tl_a, t) == pos_at(tl_b, t) {
                    out.push(ReplanConflict { a1: a, a2: b, t });
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }
            // Edge / swap conflict
            for t in 1..=max_t {
                let pa_prev = pos_at(tl_a, t - 1);
                let pa_cur = pos_at(tl_a, t);
                let pb_prev = pos_at(tl_b, t - 1);
                let pb_cur = pos_at(tl_b, t);
                if pa_prev != pa_cur && pa_prev == pb_cur && pa_cur == pb_prev {
                    out.push(ReplanConflict { a1: a, a2: b, t });
                    break;
                }
            }
        }
    }

    out
}

/// Detect every conflict on the current timelines where agent `a` is one of
/// the participants, writing them into `out`. Used by `find_consistent_paths`
/// after replanning `a`: the list is fed to `find_replan_agents` so cascading
/// replans propagate until the tree is internally consistent.
///
/// Pooled-buffer variant: the caller owns `out` (stored on `PbsPlanner`) and
/// this function `clear()`s it on entry.
fn conflicts_involving_agent_into(
    out: &mut Vec<ReplanConflict>,
    timelines: &[ArcTimeline],
    a: usize,
    window: usize,
) {
    out.clear();
    let n = timelines.len();
    if a >= n || n < 2 {
        return;
    }

    let tl_a = &timelines[a];
    let max_a = tl_a.len().saturating_sub(1);
    let max_t_a = max_a.min(window);

    for (b, tl_b) in timelines.iter().enumerate() {
        if b == a {
            continue;
        }
        let max_b = tl_b.len().saturating_sub(1);
        let max_t = max_t_a.max(max_b.min(window));

        // Vertex conflicts
        for t in 0..=max_t {
            let pa = pos_at(tl_a, t);
            let pb = pos_at(tl_b, t);
            if pa == pb {
                out.push(ReplanConflict { a1: a, a2: b, t });
                break; // one vertex conflict per agent-pair is enough
            }
        }

        // Edge conflicts (swap at consecutive timesteps)
        for t in 1..=max_t {
            let pa_prev = pos_at(tl_a, t - 1);
            let pa_cur = pos_at(tl_a, t);
            let pb_prev = pos_at(tl_b, t - 1);
            let pb_cur = pos_at(tl_b, t);
            if pa_prev != pa_cur && pa_prev == pb_cur && pa_cur == pb_prev {
                out.push(ReplanConflict { a1: a, a2: b, t });
                break;
            }
        }
    }
}

/// Remove every cached conflict record from `conflicts` that involves agent
/// `a`. Direct port of `PBS.cpp:439`'s `remove_conflicts(node->conflicts, a)`.
#[inline]
fn remove_conflicts_for_agent(conflicts: &mut Vec<ReplanConflict>, a: usize) {
    conflicts.retain(|c| c.a1 != a && c.a2 != a);
}

/// Error returned by `find_consistent_paths` when the cascade cannot converge.
/// Maps to `PBS.cpp:423` (count > 5× num agents) and `PBS.cpp:437` (child A*
/// returns empty). Callers treat either as "this branch fails".
#[derive(Debug, Clone, Copy)]
enum ConsistentPathsFail {
    /// The cascade loop ran more than `plans.len() * PBS_CONSISTENT_PATHS_REPLAN_MULT`
    /// iterations without reaching a fixed point.
    #[allow(dead_code)]
    CycleExceeded,
    /// An individual agent's A* returned no path under the current priority
    /// constraints. Equivalent to `PBS.cpp:437` (`find_path` returned false).
    #[allow(dead_code)]
    PlanFailed,
}

/// Port of `PBS.cpp:410-453` (`find_consistent_paths`). Iteratively replans
/// every agent whose path is invalidated by the most recent priority addition
/// until either (a) no new conflicts are introduced, (b) the cycle guard
/// trips, or (c) an individual A* call fails.
///
/// `initial_agent` is the agent that just had a priority constraint added
/// (equivalent to the reference's `agent` parameter). On the root-node call
/// this can be set to `None` (cascade over the existing `conflicts_seed`
/// list alone).
///
/// `conflicts_seed` is the existing list of conflicts to consider for the
/// initial `find_replan_agents` call. On child nodes it should be cloned
/// from the parent's conflict list; on the root it can be empty.
#[allow(clippy::too_many_arguments)]
fn find_consistent_paths(
    node: &mut PbsNode,
    initial_agent: Option<usize>,
    agents: &[WindowAgent],
    grid: &GridMap,
    horizon: usize,
    ci_buf: &mut FlatConstraintIndex,
    seq_stg: &mut SeqGoalGrid,
    start_constraints: &[(IVec2, u64)],
    dist_cache: &DistanceMapCache,
    conflicts_seed: Vec<ReplanConflict>,
    agent_starts: &[IVec2],
    pc_stack: &mut Vec<usize>,
    pc_seen: &mut Vec<bool>,
    scratch_new_conflicts: &mut Vec<ReplanConflict>,
) -> Result<(), ConsistentPathsFail> {
    // Canonical list of conflicts in this node — starts with the parent's
    // conflicts and is surgically updated as agents are replanned.
    let mut node_conflicts: Vec<ReplanConflict> = conflicts_seed;

    // Build the initial replan set: {initial_agent} ∪ find_replan_agents(node_conflicts).
    let mut replan: BTreeSet<usize> = BTreeSet::new();
    if let Some(a) = initial_agent {
        if a < node.plans.len() {
            replan.insert(a);
        }
    }
    find_replan_agents(
        &node.priority_pairs,
        &node.plans,
        agent_starts,
        &node_conflicts,
        PBS_PRIORITIZE_START_DEFAULT,
        &mut replan,
        pc_stack,
        pc_seen,
    );

    let mut count: usize = 0;
    let limit = node.plans.len() * PBS_CONSISTENT_PATHS_REPLAN_MULT;

    if std::env::var("MAFIS_PBS_DIAG").is_ok() {
        eprintln!(
            "  FCP_START: initial_agent={:?} seed_conflicts={} replan_init={:?}",
            initial_agent,
            node_conflicts.len(),
            replan
        );
    }

    while let Some(&a) = replan.iter().next() {
        replan.remove(&a);
        count += 1;
        if count > limit {
            if std::env::var("MAFIS_PBS_DIAG").is_ok() {
                eprintln!(
                    "  FCP_FAIL: CycleExceeded at count={} limit={} pairs={}",
                    count,
                    limit,
                    node.priority_pairs.len()
                );
            }
            return Err(ConsistentPathsFail::CycleExceeded);
        }

        // Replan agent `a` under the node's current priority pairs + plans.
        let mut goals_after_trim = 0usize;
        let new_plan = plan_agent(
            a,
            agents,
            &node.plans,
            &node.priority_pairs,
            grid,
            horizon,
            ci_buf,
            seq_stg,
            start_constraints,
            dist_cache,
            &mut goals_after_trim,
        );
        let new_plan = match new_plan {
            Some(p) => p,
            None => {
                if std::env::var("MAFIS_PBS_DIAG").is_ok() {
                    eprintln!(
                        "  FCP_FAIL: plan_agent returned None for agent {} (count={}, pairs={})",
                        a,
                        count,
                        node.priority_pairs.len()
                    );
                }
                return Err(ConsistentPathsFail::PlanFailed);
            }
        };

        node.plans[a] = Arc::new(new_plan);
        rebuild_timeline(&mut node.timelines, &node.plans, agents, a);

        // Drop stale conflicts involving `a`, then detect new ones and add
        // them back. Feed the new batch to find_replan_agents.
        remove_conflicts_for_agent(&mut node_conflicts, a);
        conflicts_involving_agent_into(scratch_new_conflicts, &node.timelines, a, horizon);
        find_replan_agents(
            &node.priority_pairs,
            &node.plans,
            agent_starts,
            scratch_new_conflicts,
            PBS_PRIORITIZE_START_DEFAULT,
            &mut replan,
            pc_stack,
            pc_seen,
        );
        node_conflicts.extend(scratch_new_conflicts.iter().copied());
    }

    // Re-compute g_val and persist the converged conflict list so that if
    // this node later becomes the parent of another try_branch call, its
    // child will inherit an up-to-date seed.
    node.g_val = sum_g_val(&node.plans);
    node.conflict_list = node_conflicts;
    Ok(())
}

// ---------------------------------------------------------------------------
// PbsPlanner
// ---------------------------------------------------------------------------

pub struct PbsPlanner {
    conflict_grid: ConflictGrid,
    ci_buf: FlatConstraintIndex,
    seq_stg: SeqGoalGrid,
    /// CAT (Conflict Avoidance Table) — kept around as a buffer. Populated on
    /// every `plan_window` so the buffer stays warm even though `plan_agent`
    /// no longer reads it (sequential-goal A* takes no CAT parameter).
    #[allow(dead_code)]
    cat: FlatCAT,
    /// If `true`, `try_branch` uses eager priority resolution via
    /// `find_consistent_paths` (reference default, PBS.cpp:477).
    /// If `false`, falls back to the MAFIS lazy mode: only the directly-
    /// constrained agent is replanned on each branch.
    eager: bool,
    /// Set of agent pairs `(min(a,b), max(a,b))` whose both-children branches
    /// have failed during this `plan_window` call. Reference: `PBS.cpp:72`
    /// (declaration) and `PBS.cpp:772` (insertion). When the next
    /// `choose_conflict` call sees a conflict whose pair is in this set, it
    /// is returned immediately regardless of timestep — surfacing
    /// unresolvable pairs earlier and pruning dead subtrees faster.
    ///
    /// Cleared at the start of every `plan_window` call (scope = one window).
    pub(super) nogood: BTreeSet<(usize, usize)>,
    /// Scratch stack reused by `would_create_cycle_buf` across every branch
    /// attempt in a single `plan_window` call. Always empty between calls.
    cycle_stack: Vec<usize>,
    /// Scratch "visited" bitset reused by `would_create_cycle_buf`. Sized
    /// per-call via `resize(needed, false)` in the helper.
    cycle_visited: Vec<bool>,
    /// Scratch stack reused by `priority_connected_buf` inside every
    /// `find_replan_agents` call during the PBS cascade. Always empty
    /// between calls.
    pc_stack: Vec<usize>,
    /// Scratch "seen" bitset reused by `priority_connected_buf`. Agent-indexed
    /// `Vec<bool>` replaces the former per-call `HashSet<usize>` alloc.
    pc_seen: Vec<bool>,
    /// Scratch buffer reused by `conflicts_involving_agent_into` inside the
    /// `find_consistent_paths` cascade loop. Cleared and repopulated on each
    /// cascade step; never leaves this file.
    scratch_new_conflicts: Vec<ReplanConflict>,
    /// Scratch buffer for the augmented PBS goal set (primary + peek-chain
    /// extensions for every agent), used to pre-populate the persistent
    /// `DistanceMapCache` at the start of every `plan_window`. Cleared and
    /// re-extended per replan instead of freshly allocated.
    scratch_unique_goal_pairs: Vec<(IVec2, IVec2)>,
    /// Scratch buffer for agent starting positions, hoisted out of the PBS
    /// DFS loop so `find_consistent_paths` doesn't re-allocate it once per
    /// branch. Cleared and re-extended per replan.
    scratch_agent_starts: Vec<IVec2>,
}

impl Default for PbsPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PbsPlanner {
    /// Construct an eager-priority PBS planner (the canonical default per
    /// `PBS_LAZY_PRIORITY_DEFAULT = false` / `driver.cpp:114`).
    ///
    /// Scratch buffers (`ci_buf`, `seq_stg`, `cat`) are constructed at minimal
    /// size and grow lazily on first `plan_window`. For production callers
    /// that know the grid dimensions ahead of time, prefer
    /// [`PbsPlanner::with_capacity`] which pre-sizes the buffers and eliminates
    /// the first-tick allocation stall.
    pub fn new() -> Self {
        Self::with_eager(true)
    }

    /// Construct an eager-priority PBS planner with **pre-sized scratch
    /// buffers**. Eliminates the ~3.2 MB allocation spike that would otherwise
    /// happen on the first `plan_window` call (which is the freeze the user
    /// experiences immediately after "Start Simulation").
    ///
    /// `max_goals` should be `PBS_GOAL_SEQUENCE_MAX_LEN + 1` (primary +
    /// peek-chain extensions) — see `windowed.rs::WindowAgent::goal_sequence`.
    ///
    /// The `*::reset` methods on the buffers are idempotent: when called with
    /// matching dimensions on the first `plan_window`, they fast-path to a
    /// `.fill()` zero pass instead of reallocating.
    pub fn with_capacity(grid_w: usize, grid_h: usize, horizon: usize, max_goals: usize) -> Self {
        let w = grid_w as i32;
        let h = grid_h as i32;
        let cells = grid_w * grid_h;
        let max_time = horizon as u64;

        let mut conflict_grid = ConflictGrid::new();
        conflict_grid.ensure_size(cells);

        let mut seq_stg = SeqGoalGrid::new();
        seq_stg.reset(w, h, max_time, max_goals);

        Self {
            conflict_grid,
            ci_buf: FlatConstraintIndex::new(w, h, max_time),
            seq_stg,
            cat: FlatCAT::new(w, h, max_time),
            eager: true,
            nogood: BTreeSet::new(),
            cycle_stack: Vec::new(),
            cycle_visited: Vec::new(),
            pc_stack: Vec::new(),
            pc_seen: Vec::new(),
            scratch_new_conflicts: Vec::new(),
            scratch_unique_goal_pairs: Vec::new(),
            scratch_agent_starts: Vec::new(),
        }
    }

    /// Construct a lazy-priority PBS planner. Kept for regression benchmarking
    /// against the previous MAFIS implementation and for tests that want to
    /// observe the delta between eager and lazy mode.
    pub fn new_lazy() -> Self {
        Self::with_eager(false)
    }

    fn with_eager(eager: bool) -> Self {
        Self {
            conflict_grid: ConflictGrid::new(),
            ci_buf: FlatConstraintIndex::new(1, 1, 1),
            seq_stg: SeqGoalGrid::new(),
            cat: FlatCAT::new(1, 1, 1),
            eager,
            nogood: BTreeSet::new(),
            cycle_stack: Vec::new(),
            cycle_visited: Vec::new(),
            pc_stack: Vec::new(),
            pc_seen: Vec::new(),
            scratch_new_conflicts: Vec::new(),
            scratch_unique_goal_pairs: Vec::new(),
            scratch_agent_starts: Vec::new(),
        }
    }
}

impl WindowedPlanner for PbsPlanner {
    fn name(&self) -> &'static str {
        "pbs"
    }

    fn plan_window(
        &mut self,
        ctx: &WindowContext,
        dist_cache: &mut DistanceMapCache,
        _rng: &mut SeededRng,
    ) -> WindowResult {
        let n = ctx.agents.len();
        if n == 0 {
            return WindowResult::Solved(Vec::new());
        }

        let cells = (ctx.grid.width * ctx.grid.height) as usize;
        self.conflict_grid.ensure_size(cells);

        // nogood lives for the duration of a single plan_window call — the
        // priority context resets across windows, so a pair that was
        // unresolvable last window may be resolvable now. Clear at entry.
        self.nogood.clear();

        // Agent starting positions — window-invariant, hoisted to a scratch
        // field on `self` so `find_consistent_paths` doesn't re-allocate it
        // once per branch and `plan_window` doesn't allocate it once per call.
        self.scratch_agent_starts.clear();
        self.scratch_agent_starts.extend(ctx.agents.iter().map(|a| a.pos));

        // TEMP DIAG (active when MAFIS_PBS_DIAG is set): counts of cascade
        // branches per plan_window call.
        let diag_enabled = std::env::var("MAFIS_PBS_DIAG").is_ok();
        let mut diag_branches_total: usize = 0;
        let mut diag_branches_ok: usize = 0;
        let mut diag_branches_fail: usize = 0;
        let mut diag_solved: bool = false;
        let mut diag_node_limit_hit: bool = false;

        // ── Pre-populate the persistent simulation distance-map cache ────
        //
        // Collect every goal cell that any branch of the PBS tree may need
        // (primary goal + peek-chain extensions for every agent) and ensure
        // they are all populated in the simulation-level `DistanceMapCache`.
        // Cache hits are free (no BFS); only first-time goals trigger BFS.
        // After this point the cache is read-only for the duration of
        // `plan_window`, so `plan_agent` takes an immutable `&DistanceMapCache`
        // borrow that coexists with the simultaneous `&mut self.seq_stg` /
        // `&mut self.ci_buf` borrows in the PBS DFS loop.
        //
        // The `pos` field of `(pos, goal)` pairs is unused for cache keying;
        // we pass each agent's actual pos for clarity.
        //
        // Reuses the scratch field on `self` instead of allocating a fresh
        // Vec per replan.
        self.scratch_unique_goal_pairs.clear();
        for agent in ctx.agents {
            self.scratch_unique_goal_pairs.push((agent.pos, agent.goal));
            for &cell in &agent.goal_sequence {
                self.scratch_unique_goal_pairs.push((agent.pos, cell));
            }
        }
        // The `Vec<&DistanceMap>` return value is dropped immediately — we
        // use `dist_cache.get_cached(goal)` inside the PBS loop, which only
        // needs `&DistanceMapCache`.
        let _ = dist_cache.get_or_compute(ctx.grid, &self.scratch_unique_goal_pairs);

        // ── Build initial plans ──────────────────────────────────────────
        //
        // Warm-start from previous plans when available; otherwise call the
        // root-only `plan_agent_root_no_reset` (sequential-goal A* with no
        // priority constraints). This deduplicates the planning code so
        // initial plans also benefit from goal sequences and the per-window
        // cache.
        //
        // **Hoisted constraint build**: at the root, every call would
        // otherwise reset `ci_buf` and re-insert the same `start_constraints`.
        // Since the constraint state is identical across all N root calls, we
        // build it once here and pass the pre-populated `ci_buf` into the
        // no-reset variant below. Saves N-1 resets + N×|start_constraints|
        // redundant `add_vertex` calls per window.
        self.ci_buf.reset(ctx.grid.width, ctx.grid.height, ctx.horizon as u64);
        for &(pos, time) in ctx.start_constraints {
            self.ci_buf.add_vertex(pos, time);
        }

        // Plans are built as owned `Vec<Action>` first, then wrapped into
        // `Arc` before entering the PBS tree so siblings can share them
        // without deep-cloning.
        //
        // **Rayon parallelism** (RHCR-PBS perf sprint 2026-04-09): at the root
        // node, every agent's plan_agent call is independent. There are no
        // priority pairs yet, so no agent's plan affects any other agent's
        // planning — the N calls can run in parallel. Shared reads:
        //   - `&self.ci_buf` — populated synchronously just above, read-only
        //     for the parallel section (the cascade loop later does its own
        //     per-agent rebuild via full `plan_agent`).
        //   - `dist_cache` — pre-populated by `get_or_compute` above, now
        //     read-only via `get_cached` inside `plan_agent_root_no_reset`.
        //   - `ctx.agents`, `ctx.grid`, `ctx.horizon` — immutable borrows.
        // Per-worker mutable state:
        //   - Each rayon worker allocates its own `SeqGoalGrid` (~3-7 MB at
        //     warehouse_single_dock dims, amortized across the ~N/num_threads calls
        //     each worker handles).
        //
        // WASM is single-threaded and cannot run rayon, so the whole `rhcr`
        // module is cfg-gated out of wasm32 builds (see `solver/mod.rs`).
        // This function only ever compiles on native.
        let initial_plans: Vec<ArcPlan> = {
            use rayon::prelude::*;
            let ci_buf_ref: &FlatConstraintIndex = &self.ci_buf;
            let dist_cache_ref: &DistanceMapCache = dist_cache;
            let agents_ref = ctx.agents;
            let grid_ref = ctx.grid;
            let horizon = ctx.horizon;
            let initial_plans_ref = ctx.initial_plans;

            (0..n)
                .into_par_iter()
                .map(|i| {
                    // Warm-start: reuse previous plan if available
                    if let Some(ref init_plan) = initial_plans_ref[i] {
                        return Arc::new(init_plan.clone());
                    }

                    // Per-worker SeqGoalGrid. On the worker's first call this
                    // allocates the full buffer; subsequent calls on the same
                    // worker reuse it via the dirty-list reset.
                    let mut local_stg = SeqGoalGrid::new();
                    let mut goals_after_trim = 0usize;
                    match plan_agent_root_no_reset(
                        i,
                        agents_ref,
                        grid_ref,
                        horizon,
                        ci_buf_ref,
                        &mut local_stg,
                        dist_cache_ref,
                        &mut goals_after_trim,
                    ) {
                        Some(p) => Arc::new(p),
                        None => Arc::new(vec![Action::Wait; horizon.min(1)]),
                    }
                })
                .collect()
        };

        // Build timelines once for initial plans
        let initial_timelines = build_timelines(&initial_plans, ctx.agents);

        // Build CAT from all initial plans for soft-constraint tie-breaking.
        self.cat.reset(ctx.grid.width, ctx.grid.height, ctx.horizon as u64);
        for (i, plan) in initial_plans.iter().enumerate() {
            self.cat.add_path(plan, ctx.agents[i].pos);
        }

        // PBS tree search — DFS with best-node tracking
        let mut dfs: Vec<PbsNode> = Vec::new();
        let mut node_count = 0usize;
        let mut best_node: Option<PbsNode> = None;

        // Build root node with earliest_collision, g_val, and (in eager mode)
        // a materialized conflict list that seeds the first cascade.
        let root_conflict_list = if self.eager {
            all_pairwise_conflicts(&initial_timelines, ctx.horizon)
        } else {
            Vec::new()
        };
        let root_conflicts_count =
            self.conflict_grid.count_conflicts(&initial_timelines, ctx.grid.width, ctx.horizon);
        let root_earliest = self
            .conflict_grid
            .detect_first(&initial_timelines, ctx.grid.width, ctx.horizon)
            .map(|c| c.time as usize)
            .unwrap_or(usize::MAX);
        let root_g_val = sum_g_val(&initial_plans);

        dfs.push(PbsNode {
            plans: initial_plans,
            timelines: initial_timelines,
            priority_pairs: Vec::new(),
            conflicts: root_conflicts_count,
            earliest_collision: root_earliest,
            conflict_list: root_conflict_list,
            g_val: root_g_val,
            id: node_count,
        });
        node_count += 1;

        let eager = self.eager;

        while let Some(node) = dfs.pop() {
            if node_count >= ctx.node_limit {
                diag_node_limit_hit = true;
                let best = best_node.unwrap_or(node);
                if diag_enabled {
                    eprintln!(
                        "PBS_DIAG: n={n} H={} W={} node_limit={} nodes={} branches={}/{}ok/{}fail solved={} limit_hit={}",
                        ctx.horizon,
                        ctx.start_constraints.len(),
                        ctx.node_limit,
                        node_count,
                        diag_branches_total,
                        diag_branches_ok,
                        diag_branches_fail,
                        diag_solved,
                        diag_node_limit_hit,
                    );
                }
                return to_partial_result(best.plans, ctx.agents);
            }

            // Update best node — reference tie-break (PBS.cpp:630):
            //     (earliest_collision DESC, f_val/g_val ASC)
            // where "later earliest_collision" means the node delays the first
            // conflict further, and "lower g_val" means cheaper sum-of-costs.
            let dominated = match &best_node {
                Some(bn) => {
                    node.earliest_collision > bn.earliest_collision
                        || (node.earliest_collision == bn.earliest_collision
                            && node.g_val < bn.g_val)
                }
                None => true,
            };
            if dominated {
                best_node = Some(PbsNode {
                    plans: node.plans.clone(),
                    timelines: node.timelines.clone(),
                    priority_pairs: node.priority_pairs.clone(),
                    conflicts: node.conflicts,
                    earliest_collision: node.earliest_collision,
                    conflict_list: node.conflict_list.clone(),
                    g_val: node.g_val,
                    id: node.id,
                });
            }

            // choose_conflict: the earliest-time conflict by default, with
            // the nogood override (PBS.cpp:274-290). If any active conflict
            // pair matches a nogood entry, return that conflict so the
            // known-unresolvable subtree is pruned immediately.
            let conflict = self.conflict_grid.detect_with_nogood(
                &node.timelines,
                ctx.grid.width,
                ctx.horizon,
                &self.nogood,
            );

            if let Some(conflict) = conflict {
                let child1 = try_branch(
                    &node,
                    conflict.agent_a,
                    conflict.agent_b,
                    ctx.agents,
                    ctx.grid,
                    ctx.horizon,
                    &mut self.ci_buf,
                    &mut self.seq_stg,
                    ctx.start_constraints,
                    dist_cache,
                    eager,
                    &self.scratch_agent_starts,
                    &mut self.cycle_stack,
                    &mut self.cycle_visited,
                    &mut self.pc_stack,
                    &mut self.pc_seen,
                    &mut self.scratch_new_conflicts,
                );
                diag_branches_total += 1;
                if child1.is_some() {
                    diag_branches_ok += 1;
                } else {
                    diag_branches_fail += 1;
                }
                let child2 = try_branch(
                    &node,
                    conflict.agent_b,
                    conflict.agent_a,
                    ctx.agents,
                    ctx.grid,
                    ctx.horizon,
                    &mut self.ci_buf,
                    &mut self.seq_stg,
                    ctx.start_constraints,
                    dist_cache,
                    eager,
                    &self.scratch_agent_starts,
                    &mut self.cycle_stack,
                    &mut self.cycle_visited,
                    &mut self.pc_stack,
                    &mut self.pc_seen,
                    &mut self.scratch_new_conflicts,
                );
                diag_branches_total += 1;
                if child2.is_some() {
                    diag_branches_ok += 1;
                } else {
                    diag_branches_fail += 1;
                }

                // If both children failed: record the conflict pair in
                // nogood (PBS.cpp:770-772) so any future reachable node
                // that re-encounters this pair prunes faster.
                if child1.is_none() && child2.is_none() {
                    let pair = (
                        conflict.agent_a.min(conflict.agent_b),
                        conflict.agent_a.max(conflict.agent_b),
                    );
                    self.nogood.insert(pair);
                }

                // Push worse child first, better child second (better popped first in DFS)
                let mut children: Vec<PbsNode> = Vec::new();
                #[allow(clippy::manual_flatten)]
                for child_opt in [child1, child2] {
                    if let Some(mut child) = child_opt {
                        // `count_conflicts` counts collision *events* (not pairs),
                        // so we keep the grid scan for the tie-break field.
                        // `earliest_collision`, however, is the min-timestep across
                        // conflicts, and in eager mode `find_consistent_paths`
                        // leaves `child.conflict_list` populated with the converged
                        // per-pair conflicts — each entry's `t` is the pair's
                        // first conflict, so the min over the list equals the
                        // global earliest conflict event. Derive it in O(|list|)
                        // instead of re-scanning the full grid.
                        let c_conflicts = self.conflict_grid.count_conflicts(
                            &child.timelines,
                            ctx.grid.width,
                            ctx.horizon,
                        );
                        let c_earliest = if eager {
                            child.conflict_list.iter().map(|c| c.t).min().unwrap_or(usize::MAX)
                        } else {
                            self.conflict_grid
                                .detect_first(&child.timelines, ctx.grid.width, ctx.horizon)
                                .map(|c| c.time as usize)
                                .unwrap_or(usize::MAX)
                        };
                        child.conflicts = c_conflicts;
                        child.earliest_collision = c_earliest;
                        child.g_val = sum_g_val(&child.plans);
                        child.id = node_count;
                        node_count += 1;
                        children.push(child);
                    }
                }

                // Reference push order (PBS.cpp:750): the better child must
                // be popped first by DFS. Vec::pop returns the last-pushed
                // element, so we sort WORST → BEST and push in order. Ranking
                // (reference): primary `f_val` (= `g_val` here, ASC), tie-break
                // `num_of_collisions` (ASC). The sort below ranks descending
                // on that tuple so index 0 = worst, index 1 = best.
                children.sort_by(|a, b| {
                    b.g_val.cmp(&a.g_val).then_with(|| b.conflicts.cmp(&a.conflicts))
                });
                for child in children {
                    dfs.push(child);
                }
            } else {
                // No conflicts — solution found
                diag_solved = true;
                if diag_enabled {
                    eprintln!(
                        "PBS_DIAG: n={n} H={} nodes={} branches={}/{}ok/{}fail solved={} limit_hit={}",
                        ctx.horizon,
                        node_count,
                        diag_branches_total,
                        diag_branches_ok,
                        diag_branches_fail,
                        diag_solved,
                        diag_node_limit_hit,
                    );
                }
                return to_window_result(node.plans, ctx.agents);
            }
        }

        if diag_enabled {
            eprintln!(
                "PBS_DIAG: n={n} H={} nodes={} branches={}/{}ok/{}fail solved={} limit_hit={}",
                ctx.horizon,
                node_count,
                diag_branches_total,
                diag_branches_ok,
                diag_branches_fail,
                diag_solved,
                diag_node_limit_hit,
            );
        }

        // No solution found — return best partial
        match best_node {
            Some(best) => to_partial_result(best.plans, ctx.agents),
            None => WindowResult::Partial { solved: Vec::new(), failed: (0..n).collect() },
        }
    }
}

/// Check if adding edge (higher → lower) to the priority pairs creates a cycle.
/// Pooled-buffer variant for the hot path: caller owns `stack` and `visited`
/// Vecs (stored on `PbsPlanner`) to avoid per-call allocation.
fn would_create_cycle_buf(
    stack: &mut Vec<usize>,
    visited: &mut Vec<bool>,
    pairs: &[(usize, usize)],
    higher: usize,
    lower: usize,
) -> bool {
    let needed = pairs.len().max(higher + 1).max(lower + 1);
    stack.clear();
    visited.clear();
    visited.resize(needed, false);
    stack.push(lower);
    visited[lower] = true;

    while let Some(node) = stack.pop() {
        for &(h, l) in pairs {
            if h == node && !visited.get(l).copied().unwrap_or(false) {
                if l == higher {
                    return true;
                }
                if l < visited.len() {
                    visited[l] = true;
                    stack.push(l);
                }
            }
        }
    }
    false
}

/// Allocating convenience wrapper — used by unit tests. The hot path uses
/// `would_create_cycle_buf` with buffers owned by `PbsPlanner`.
#[cfg(test)]
fn would_create_cycle(pairs: &[(usize, usize)], higher: usize, lower: usize) -> bool {
    let mut stack = Vec::new();
    let mut visited = Vec::new();
    would_create_cycle_buf(&mut stack, &mut visited, pairs, higher, lower)
}

#[allow(clippy::too_many_arguments)]
fn try_branch(
    parent: &PbsNode,
    higher: usize,
    lower: usize,
    agents: &[WindowAgent],
    grid: &GridMap,
    horizon: usize,
    ci_buf: &mut FlatConstraintIndex,
    seq_stg: &mut SeqGoalGrid,
    start_constraints: &[(IVec2, u64)],
    dist_cache: &DistanceMapCache,
    eager: bool,
    agent_starts: &[IVec2],
    cycle_stack: &mut Vec<usize>,
    cycle_visited: &mut Vec<bool>,
    pc_stack: &mut Vec<usize>,
    pc_seen: &mut Vec<bool>,
    scratch_new_conflicts: &mut Vec<ReplanConflict>,
) -> Option<PbsNode> {
    if would_create_cycle_buf(cycle_stack, cycle_visited, &parent.priority_pairs, higher, lower) {
        return None;
    }

    let mut new_pairs = parent.priority_pairs.clone();
    new_pairs.push((higher, lower));

    let mut new_plans = parent.plans.clone();
    let mut new_timelines = parent.timelines.clone();

    if eager {
        // ── Eager mode (PBS.cpp:506-515) ────────────────────────────────
        //
        // Build the child node inheriting the parent's conflict list, then
        // cascade via `find_consistent_paths`: replan every agent whose path
        // is invalidated by the new priority constraint until we reach a
        // fixed point (or a cycle/failure). The child is constructed with an
        // empty `conflict_list` because `find_consistent_paths` overwrites it
        // with the converged list on success; the parent's list is passed
        // directly as the `conflicts_seed` argument to seed the cascade.
        let mut child = PbsNode {
            plans: new_plans,
            timelines: new_timelines,
            priority_pairs: new_pairs,
            conflicts: 0,
            earliest_collision: 0,
            conflict_list: Vec::new(),
            g_val: 0,
            id: 0,
        };
        match find_consistent_paths(
            &mut child,
            Some(lower),
            agents,
            grid,
            horizon,
            ci_buf,
            seq_stg,
            start_constraints,
            dist_cache,
            parent.conflict_list.clone(),
            agent_starts,
            pc_stack,
            pc_seen,
            scratch_new_conflicts,
        ) {
            Ok(()) => Some(child),
            Err(_) => None,
        }
    } else {
        // ── Lazy mode (PBS.cpp:478-504) — MAFIS's pre-Stream-C path ─────
        let mut goals_after_trim = 0usize;
        if let Some(new_plan) = plan_agent(
            lower,
            agents,
            &new_plans,
            &new_pairs,
            grid,
            horizon,
            ci_buf,
            seq_stg,
            start_constraints,
            dist_cache,
            &mut goals_after_trim,
        ) {
            new_plans[lower] = Arc::new(new_plan);
            rebuild_timeline(&mut new_timelines, &new_plans, agents, lower);
            Some(PbsNode {
                plans: new_plans,
                timelines: new_timelines,
                priority_pairs: new_pairs,
                conflicts: 0,
                earliest_collision: 0,
                conflict_list: Vec::new(),
                g_val: 0,
                id: 0,
            })
        } else {
            None
        }
    }
}

/// Cheaply unwrap an `Arc<Vec<Action>>` into an owned `Vec<Action>`. In the
/// common case (final solve — all sibling DFS entries have been popped by the
/// time we reach here), the refcount is 1 and `try_unwrap` succeeds in O(1).
/// If another `Arc` still references the inner Vec (e.g., a still-live best-
/// node snapshot when returning via `to_partial_result`), we fall back to a
/// deep clone, matching the pre-Arc semantics.
#[inline]
fn arc_plan_into_vec(arc: ArcPlan) -> Vec<Action> {
    Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone())
}

fn to_window_result(plans: Vec<ArcPlan>, agents: &[WindowAgent]) -> WindowResult {
    let fragments: Vec<PlanFragment> = plans
        .into_iter()
        .zip(agents.iter())
        .map(|(plan, agent)| PlanFragment {
            agent_index: agent.index,
            actions: arc_plan_into_vec(plan).into_iter().collect(),
        })
        .collect();
    WindowResult::Solved(fragments)
}

fn to_partial_result(plans: Vec<ArcPlan>, agents: &[WindowAgent]) -> WindowResult {
    let mut solved = Vec::new();
    let mut failed = Vec::new();

    for (plan, agent) in plans.into_iter().zip(agents.iter()) {
        if plan.is_empty() || plan.iter().all(|a| *a == Action::Wait) {
            failed.push(agent.index);
        } else {
            solved.push(PlanFragment {
                agent_index: agent.index,
                actions: arc_plan_into_vec(plan).into_iter().collect(),
            });
        }
    }

    if failed.is_empty() {
        WindowResult::Solved(solved)
    } else {
        WindowResult::Partial { solved, failed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::solver::shared::heuristics::{DistanceMap, DistanceMapCache};
    use smallvec::SmallVec;

    /// Owns the backing storage that `WindowContext` borrows. Tests build
    /// one of these per call site, then pass it into `make_ctx`. After the
    /// `WindowContext` move to borrowed slices, the test fixtures can no
    /// longer construct the data inline inside `make_ctx`.
    struct CtxOwned {
        initial_plans: Vec<Option<Vec<Action>>>,
        start_constraints: Vec<(IVec2, u64)>,
    }

    fn ctx_owned(agents: &[WindowAgent]) -> CtxOwned {
        CtxOwned {
            initial_plans: vec![None; agents.len()],
            start_constraints: agents.iter().map(|a| (a.pos, 0u64)).collect(),
        }
    }

    fn make_ctx<'a>(
        grid: &'a GridMap,
        agents: &'a [WindowAgent],
        dist_maps: &'a [&'a DistanceMap],
        owned: &'a CtxOwned,
    ) -> WindowContext<'a> {
        WindowContext {
            grid,
            horizon: 20,
            node_limit: 500,
            agents,
            distance_maps: dist_maps,
            initial_plans: &owned.initial_plans,
            start_constraints: &owned.start_constraints,
            travel_penalties: &[],
        }
    }

    #[test]
    fn cycle_detection_no_cycle() {
        let pairs = vec![(0, 1), (1, 2)];
        assert!(!would_create_cycle(&pairs, 0, 2));
        assert!(!would_create_cycle(&pairs, 2, 3));
    }

    #[test]
    fn cycle_detection_direct_cycle() {
        let pairs = vec![(0, 1)];
        assert!(would_create_cycle(&pairs, 1, 0));
    }

    #[test]
    fn cycle_detection_transitive_cycle() {
        let pairs = vec![(0, 1), (1, 2)];
        assert!(would_create_cycle(&pairs, 2, 0));
    }

    #[test]
    fn cycle_detection_empty_pairs() {
        assert!(!would_create_cycle(&[], 0, 1));
    }

    #[test]
    fn pbs_empty_agents() {
        let grid = GridMap::new(5, 5);
        let owned = ctx_owned(&[]);
        let ctx = make_ctx(&grid, &[], &[], &owned);
        let mut planner = PbsPlanner::new();
        let mut dist_cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut dist_cache, &mut rng);
        assert!(matches!(result, WindowResult::Solved(v) if v.is_empty()));
    }

    #[test]
    fn pbs_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let agents = vec![WindowAgent {
            index: 0,
            pos: IVec2::ZERO,
            goal: IVec2::new(4, 4),
            goal_sequence: SmallVec::new(),
        }];
        let dm = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm];
        let owned = ctx_owned(&agents);
        let ctx = make_ctx(&grid, &agents, &dist_maps, &owned);
        let mut planner = PbsPlanner::new();
        let mut dist_cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut dist_cache, &mut rng);
        match result {
            WindowResult::Solved(frags) => {
                assert_eq!(frags.len(), 1);
                assert!(!frags[0].actions.is_empty());
            }
            _ => panic!("expected Solved"),
        }
    }

    #[test]
    fn pbs_two_agents_no_conflict() {
        let grid = GridMap::new(5, 5);
        let agents = vec![
            WindowAgent {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: IVec2::new(4, 0),
                goal_sequence: SmallVec::new(),
            },
            WindowAgent {
                index: 1,
                pos: IVec2::new(0, 4),
                goal: IVec2::new(4, 4),
                goal_sequence: SmallVec::new(),
            },
        ];
        let dm0 = DistanceMap::compute(&grid, IVec2::new(4, 0));
        let dm1 = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm0, &dm1];
        let owned = ctx_owned(&agents);
        let ctx = make_ctx(&grid, &agents, &dist_maps, &owned);
        let mut planner = PbsPlanner::new();
        let mut dist_cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut dist_cache, &mut rng);
        assert!(matches!(result, WindowResult::Solved(_)));
    }

    #[test]
    fn conflict_grid_detects_vertex_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5×5

        // Two agents at same position at t=1
        let timelines: Vec<ArcTimeline> = vec![
            Arc::new(vec![IVec2::new(0, 0), IVec2::new(1, 0)]),
            Arc::new(vec![IVec2::new(2, 0), IVec2::new(1, 0)]),
        ];
        let conflict = cg.detect_first(&timelines, 5, usize::MAX);
        assert!(conflict.is_some());
    }

    #[test]
    fn conflict_grid_detects_edge_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5×5

        // Two agents swap positions
        let timelines: Vec<ArcTimeline> = vec![
            Arc::new(vec![IVec2::new(0, 0), IVec2::new(1, 0)]),
            Arc::new(vec![IVec2::new(1, 0), IVec2::new(0, 0)]),
        ];
        let conflict = cg.detect_first(&timelines, 5, usize::MAX);
        assert!(conflict.is_some());
    }

    #[test]
    fn conflict_grid_no_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25);

        let timelines: Vec<ArcTimeline> = vec![
            Arc::new(vec![IVec2::new(0, 0), IVec2::new(1, 0)]),
            Arc::new(vec![IVec2::new(0, 4), IVec2::new(1, 4)]),
        ];
        assert!(cg.detect_first(&timelines, 5, usize::MAX).is_none());
    }

    #[test]
    fn pbs_finds_solution_with_tight_node_limit() {
        let grid = GridMap::new(5, 5);
        let agents = vec![
            WindowAgent {
                index: 0,
                pos: IVec2::new(0, 2),
                goal: IVec2::new(4, 2),
                goal_sequence: SmallVec::new(),
            },
            WindowAgent {
                index: 1,
                pos: IVec2::new(4, 2),
                goal: IVec2::new(0, 2),
                goal_sequence: SmallVec::new(),
            },
        ];
        let dm0 = DistanceMap::compute(&grid, IVec2::new(4, 2));
        let dm1 = DistanceMap::compute(&grid, IVec2::new(0, 2));
        let dist_maps: Vec<&DistanceMap> = vec![&dm0, &dm1];
        let initial_plans: Vec<Option<Vec<Action>>> = vec![None; agents.len()];
        let start_constraints: Vec<(IVec2, u64)> = agents.iter().map(|a| (a.pos, 0u64)).collect();
        let ctx = WindowContext {
            grid: &grid,
            horizon: 12,
            node_limit: 6,
            agents: &agents,
            distance_maps: &dist_maps,
            initial_plans: &initial_plans,
            start_constraints: &start_constraints,
            travel_penalties: &[],
        };
        let mut planner = PbsPlanner::new();
        let mut dist_cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut dist_cache, &mut rng);
        assert!(
            matches!(result, WindowResult::Solved(_)),
            "DFS should solve 2 crossing agents within 6 nodes"
        );
    }

    #[test]
    fn conflict_grid_respects_window_scope() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5x5

        // Two agents that collide at t=2 (both at position (2,0))
        let timelines: Vec<ArcTimeline> = vec![
            Arc::new(vec![IVec2::new(0, 0), IVec2::new(1, 0), IVec2::new(2, 0)]),
            Arc::new(vec![IVec2::new(4, 0), IVec2::new(3, 0), IVec2::new(2, 0)]),
        ];
        // Full window: should detect conflict at t=2
        assert!(cg.detect_first(&timelines, 5, usize::MAX).is_some());
        // Window=1: only check t=0..1, no conflict
        assert!(cg.detect_first(&timelines, 5, 1).is_none());
    }

    // ── plan_agent (sequential-goal) tests ────────────────────────────────

    /// Helper: build a fresh planner and run `plan_agent` on a single agent
    /// with the given goal sequence on a 60×1 corridor. Returns
    /// `(plan, goals_after_trim, compute_count_after_populate)`.
    fn run_plan_agent_corridor(
        grid: &GridMap,
        start: IVec2,
        primary_goal: IVec2,
        sequence: &[IVec2],
        horizon: usize,
    ) -> (Option<Vec<Action>>, usize, usize) {
        let mut goal_sequence: SmallVec<[IVec2; 8]> = SmallVec::new();
        for &g in sequence {
            goal_sequence.push(g);
        }
        let agents = vec![WindowAgent { index: 0, pos: start, goal: primary_goal, goal_sequence }];

        let mut planner = PbsPlanner::new();
        let mut dist_cache = DistanceMapCache::default();
        // Pre-populate the persistent simulation cache with primary + chain
        // goals; pos field is unused for keying so any value works.
        let mut goal_pairs: Vec<(IVec2, IVec2)> = vec![(start, primary_goal)];
        for &g in sequence {
            goal_pairs.push((start, g));
        }
        let _ = dist_cache.get_or_compute(grid, &goal_pairs);
        // Compute count = number of unique goals in the cache after populate.
        let compute_count = dist_cache.len();

        // Cross-window start constraint at t=0 (just this agent's start cell).
        let start_constraints = vec![(start, 0u64)];

        let mut goals_after_trim = 0usize;
        let dummy_plans: [ArcPlan; 1] = [Arc::new(Vec::new())];
        let plan = plan_agent(
            0,
            &agents,
            &dummy_plans, // dummy plans for the single agent
            &[],          // no priority constraints
            grid,
            horizon,
            &mut planner.ci_buf,
            &mut planner.seq_stg,
            &start_constraints,
            &dist_cache,
            &mut goals_after_trim,
        );
        (plan, goals_after_trim, compute_count)
    }

    /// Apply a sequence of actions to a starting position and return the final cell.
    fn apply_plan(start: IVec2, plan: &[Action]) -> IVec2 {
        let mut pos = start;
        for &a in plan {
            pos = a.apply(pos);
        }
        pos
    }

    /// Easy case — primary + chain entirely fits in the horizon. Trim must
    /// NOT degrade to single-goal: `goals_after_trim` must equal the full
    /// goal-vector length so the chain stays intact.
    #[test]
    fn plan_agent_far_goal_with_sequence_returns_partial_progress() {
        // 60×1 corridor — distance from (0,0) to (k,0) is unambiguous.
        let grid = GridMap::new(60, 1);
        let start = IVec2::new(0, 0);
        let primary = IVec2::new(5, 0);
        let chain = [IVec2::new(7, 0), IVec2::new(9, 0)];
        let horizon = 15usize;

        let (plan, goals_after_trim, _compute_count) =
            run_plan_agent_corridor(&grid, start, primary, &chain, horizon);

        let plan = plan.expect("plan_agent must return a plan when primary is reachable");

        // Plan length must respect the horizon and the actual travel distance.
        assert!(plan.len() <= horizon, "plan length {} exceeds horizon {}", plan.len(), horizon);

        // The agent should make positive progress toward the primary goal.
        let dm_primary = DistanceMap::compute(&grid, primary);
        let end_pos = apply_plan(start, &plan);
        let start_dist = dm_primary.get(start);
        let end_dist = dm_primary.get(end_pos);
        assert!(
            end_dist < start_dist,
            "agent did not make progress: start_dist={start_dist}, end_dist={end_dist}, end_pos={end_pos:?}, plan={plan:?}"
        );

        // The full chain should fit in horizon=15 (Manhattan total = 9), so
        // the trim loop should NOT have to drop any goal. This guards against
        // a regression where trim degrades to single-goal in the easy case.
        assert!(
            goals_after_trim >= 2,
            "trim degraded to {} goals on the easy case (expected >= 2)",
            goals_after_trim
        );
        assert_eq!(goals_after_trim, 1 + chain.len(), "easy case must retain primary + full chain");
    }

    /// Hard case — the second goal is on the far side of a wall (unreachable
    /// from anywhere on the agent's side). The trim loop must drop it and
    /// fall back to the reachable primary goal.
    #[test]
    fn plan_agent_unreachable_second_goal_falls_back_to_first() {
        // 1D corridor with a wall at x=10. Cells 0..=9 are reachable from
        // (0,0); cells 11..=19 are reachable only from the far side.
        let mut grid = GridMap::new(20, 1);
        grid.set_obstacle(IVec2::new(10, 0));

        let start = IVec2::new(0, 0);
        let primary = IVec2::new(5, 0); // reachable
        let chain = [IVec2::new(15, 0)]; // unreachable from start
        let horizon = 30usize;

        let (plan, _goals_after_trim, _compute_count) =
            run_plan_agent_corridor(&grid, start, primary, &chain, horizon);

        let plan = plan.expect("plan_agent must return a plan after trimming the unreachable goal");

        // Sequential A* now provides horizon-bounded best-effort: when the
        // chain extension is unreachable, the search still returns the
        // best path toward the next reachable sub-goal (the primary). The
        // plan_agent trim loop may or may not engage depending on whether
        // the unreachable-goal layer drains the open set before best-effort
        // kicks in — we no longer assert on the trim count.

        // The path should make meaningful progress toward the primary goal.
        // With best-effort sequential A*, after reaching primary=(5,0) the
        // search continues toward chain[0]=(15,0) until the wall blocks it,
        // so the agent may end at any cell in the (0..10) walkable range —
        // the exact position depends on how far the open set drained toward
        // the unreachable chain goal. We only require that the agent reached
        // or passed the primary (x >= 5), i.e. it did NOT get stuck short.
        let end_pos = apply_plan(start, &plan);
        assert!(
            end_pos.x >= primary.x && end_pos.x < 10,
            "agent must reach or pass primary {primary:?} without crossing the wall (x=10), got {end_pos:?}"
        );
    }

    /// `DistanceMapCache::get_or_compute` is the single point of BFS
    /// computation for the augmented PBS goal set. Two agents heading to the
    /// same goal must trigger only one BFS, not two. The cache persists
    /// across `plan_window` calls so subsequent calls hit the cache.
    #[test]
    fn plan_agent_caches_distance_maps_within_window() {
        let grid = GridMap::new(10, 10);
        let mut cache = DistanceMapCache::default();

        // Two agents share goal (8,8); a third has a unique goal (2,2).
        // The `pos` field is unused for cache keying.
        let pairs = vec![
            (IVec2::ZERO, IVec2::new(8, 8)),
            (IVec2::ZERO, IVec2::new(8, 8)),
            (IVec2::ZERO, IVec2::new(2, 2)),
        ];
        let _ = cache.get_or_compute(&grid, &pairs);

        // Three goals listed but only two unique → exactly two cache entries.
        assert_eq!(
            cache.len(),
            2,
            "DistanceMapCache must dedupe duplicate goals: got {} entries",
            cache.len()
        );

        // Re-querying the same goals must not grow the cache (idempotent).
        let _ = cache.get_or_compute(&grid, &pairs);
        assert_eq!(
            cache.len(),
            2,
            "re-query of cached goals must be a no-op (cache size unchanged)"
        );

        // Adding a new unique goal grows the cache by one.
        let _ = cache.get_or_compute(&grid, &[(IVec2::ZERO, IVec2::new(5, 5))]);
        assert_eq!(cache.len(), 3);

        // Persistence across "windows": a new query touching only old goals
        // must be free (no new BFS).
        let _ = cache.get_or_compute(&grid, &[(IVec2::ZERO, IVec2::new(8, 8))]);
        assert_eq!(cache.len(), 3, "cached goals must not be re-computed");
    }

    /// Regression: across many simulated replans, the persistent cache must
    /// not grow unboundedly when the goal set is bounded by the topology.
    #[test]
    fn dist_cache_bounded_across_repeated_replans() {
        let grid = GridMap::new(10, 10);
        let mut cache = DistanceMapCache::default();

        // Simulate 1000 "replans" where agents always pick from a fixed
        // pool of 5 unique goal cells. Cache size must converge to 5.
        let goals = [
            IVec2::new(0, 0),
            IVec2::new(9, 0),
            IVec2::new(0, 9),
            IVec2::new(9, 9),
            IVec2::new(5, 5),
        ];
        for _ in 0..1000 {
            let pairs: Vec<(IVec2, IVec2)> = goals.iter().map(|&g| (IVec2::ZERO, g)).collect();
            let _ = cache.get_or_compute(&grid, &pairs);
        }
        assert_eq!(
            cache.len(),
            5,
            "cache must converge to bounded goal set size, got {}",
            cache.len()
        );
    }

    /// Determinism: two identical `plan_agent` invocations on the same
    /// constraint state must produce byte-identical plans. Distance-map
    /// computation is fully deterministic; A* tie-breaking is fully
    /// deterministic; the test guards against accidental nondeterminism
    /// creeping into either path.
    #[test]
    fn plan_agent_deterministic_across_runs() {
        let grid = GridMap::new(20, 1);
        let start = IVec2::new(0, 0);
        let primary = IVec2::new(5, 0);
        let chain = [IVec2::new(7, 0), IVec2::new(9, 0)];
        let horizon = 15usize;

        let (plan_a, trim_a, _) = run_plan_agent_corridor(&grid, start, primary, &chain, horizon);
        let (plan_b, trim_b, _) = run_plan_agent_corridor(&grid, start, primary, &chain, horizon);

        let plan_a = plan_a.expect("first run must produce a plan");
        let plan_b = plan_b.expect("second run must produce a plan");

        assert_eq!(plan_a, plan_b, "plan_agent must be deterministic across identical inputs");
        assert_eq!(trim_a, trim_b, "trim count must be deterministic across identical inputs");
    }
}
