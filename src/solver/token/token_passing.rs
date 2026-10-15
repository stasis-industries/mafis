//! Token Passing — decentralized lifelong MAPF solver.
//!
//! REFERENCE: docs/papers_codes/pibt/pibt2/src/tp.cpp (Okumura et al., pibt2)
//!            docs/papers_codes/pibt/pibt2/include/tp.hpp
//! Paper: Ma, Li, Kumar, Koenig — "Lifelong Multi-Agent Path Finding for Online
//! Pickup and Delivery Tasks", AAMAS 2017 (Algorithm 1, TP).
//!
//! Ported 2026-10-15 against pibt2 `tp.cpp`: task selection, endpoint rule,
//! parking and `checkAstarFin`. The deviations below come from MAFIS's task
//! model (queues, loading dwell), not from the planner.
//!
//! ## Structural mapping (MAFIS ↔ pibt2)
//!
//! | pibt2 (absolute time)              | MAFIS (relative time)                  |
//! |------------------------------------|----------------------------------------|
//! | `TOKEN[i]: vector<Node>` grows     | `Token::paths[i]: VecDeque<IVec2>`     |
//! | `current_timestep` increments      | `Token::advance()` pops front each tick|
//! | `TOKEN[i][current_t]` = current pos| `paths[i].front()` = current pos       |
//! | `TOKEN[i].size()-1 == current_t`   | `paths[i].len() <= 1` (no future plan) |
//! | `CONFLICT_TABLE[t][cell] != NIL`   | `MasterConstraintIndex::is_vertex_blocked(cell, t)` |
//! | endpoint blocking via `token_endpoints[v]` + `TOKEN[k].size()-1 < t` | `MasterConstraintIndex::add_path` extends final-cell vertex constraints to `max_time` |
//! | `checkAstarFin: g + current_t > max_constraint_time` | `ArriveAfter` blocks the goal cell up to the last time another token path uses it |
//! | task selection (tp.cpp:64-96) | `core::task::TokenPassingTaskSelection`, used via `LifelongSolver::task_scheduler` |
//! | `updatePath2` parking (tp.cpp:99-123, 206-248) | `free_parking` + `drain_goal_overrides` |
//!
//! ## Algorithm TP in MAFIS
//!
//! - **Task selection** (Alg. 1 l. 7-12): TP replaces the configured scheduler.
//!   Agents take tasks in index order; a task is skipped if its pickup is the
//!   end of another agent's path; the nearest pickup wins.
//! - **No eligible task** (l. 13-16): a free agent stays put, unless it stands
//!   on a delivery endpoint; then it moves to the nearest free parking cell
//!   (non-task endpoint, `ZoneType::Parking`).
//! - **Paths** (`updatePath`): spacetime A* against every other token path;
//!   the agent may only finish at its goal after the last time another path
//!   passes through it, and never where another path ends.
//!
//! ## Deviations from Ma et al. 2017 / pibt2 (MAFIS task model)
//!
//! 1. **Relative vs absolute time** — semantically equivalent (see mapping
//!    table). Relative time keeps memory bounded in lifelong runs.
//! 2. **One leg per path, not pickup→delivery in one path** (`updatePath1`).
//!    MAFIS agents dwell at the pickup (`Loading`) and the queue manager picks
//!    the delivery queue only then, so the delivery is unknown at selection.
//! 3. **Delivery side of the endpoint rule** — tasks have no delivery when
//!    selected; docks are serialised by the shared queue manager instead
//!    (one agent at a dock, up to 4 waiting). Queue cells are not endpoints.
//! 4. **Task set** — one random candidate task per free agent per tick (the
//!    closest scheduler's pool), not an accumulating MAPD task set.
//! 5. **Distance for task choice** — Manhattan, pibt2 uses BFS distance.
//!    Parking choice uses BFS distance as in pibt2.
//! 6. **Spawn** — agents start on corridor cells, not on non-task endpoints.
//! 7. **Planning failure** — A* is capped (`ASTAR_MAX_EXPANSIONS`,
//!    `TOKEN_ASTAR_MAX_TIME`); on failure the agent waits and retries next
//!    tick (pibt2 halts). No parking cell free → the agent stays (pibt2 halts:
//!    "not well-formed").
//! 8. **Faults** — token reset when an agent's position or goal drifts from
//!    its token (latency, collisions, queue goal changes); dead agents are
//!    obstacles, parking cells they block are skipped.
//! 9. **Well-formedness** — `core::topology::check_well_formed`. Only SD-w3
//!    satisfies Ma et al.'s condition 3; SD-w1, SD-w2 and DD do not (pickups
//!    line their aisles), so TP's completeness guarantee does not transfer.
//!    `max_agents` = number of parking cells (condition 2).

use bevy::prelude::*;
use smallvec::smallvec;
use std::collections::VecDeque;

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;

use super::common::{MasterConstraintIndex, Token};
use crate::solver::lifelong::{
    AgentPlan, AgentRestoreState, AgentState, LifelongSolver, SolverContext, StepResult,
};
use crate::solver::shared::astar::{ConstraintChecker, SpacetimeGrid, spacetime_astar_fast};
use crate::solver::shared::heuristics::{DistanceMap, DistanceMapCache};
use crate::solver::shared::traits::{Optimality, Scalability, SolverInfo};

use crate::constants::{ASTAR_MAX_EXPANSIONS, TOKEN_ASTAR_MAX_TIME, TOKEN_PATH_MAX_TIME};

// ---------------------------------------------------------------------------
// Token Passing Solver
// ---------------------------------------------------------------------------

pub struct TokenPassingSolver {
    token: Token,
    plan_buffer: Vec<AgentPlan>,
    agent_map: Vec<usize>,
    reverse_map: Vec<usize>,
    last_agent_indices: Vec<usize>,
    initialized: bool,
    master_ci: MasterConstraintIndex,
    stg: SpacetimeGrid,
    /// Reusable set tracking which agents were replanned this tick (avoids per-tick HashSet alloc).
    replanned_set: std::collections::HashSet<usize>,
    /// Free agents sent to a parking endpoint this step (applied by the runner).
    goal_overrides: Vec<(usize, IVec2)>,
}

/// pibt2 `checkAstarFin` (tp.cpp:270-272): the goal cell may only be entered
/// after `after`, the last time another token path passes through it.
struct ArriveAfter<'a> {
    inner: &'a MasterConstraintIndex,
    goal: IVec2,
    after: Option<u64>,
}

impl ConstraintChecker for ArriveAfter<'_> {
    fn is_vertex_blocked(&self, pos: IVec2, time: u64) -> bool {
        (pos == self.goal && self.after.is_some_and(|a| time <= a))
            || self.inner.is_vertex_blocked(pos, time)
    }

    fn is_edge_blocked(&self, from: IVec2, to: IVec2, time: u64) -> bool {
        self.inner.is_edge_blocked(from, to, time)
    }
}

impl Default for TokenPassingSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenPassingSolver {
    pub fn new() -> Self {
        Self {
            token: Token::new(),
            plan_buffer: Vec::new(),
            agent_map: Vec::new(),
            reverse_map: Vec::new(),
            last_agent_indices: Vec::new(),
            initialized: false,
            master_ci: MasterConstraintIndex::new(),
            stg: SpacetimeGrid::new(),
            replanned_set: std::collections::HashSet::new(),
            goal_overrides: Vec::new(),
        }
    }

    fn ensure_initialized(&mut self, agents: &[AgentState]) {
        let same_set = self.initialized
            && self.last_agent_indices.len() == agents.len()
            && self.last_agent_indices.iter().zip(agents.iter()).all(|(&a, b)| a == b.index);
        if same_set {
            return;
        }

        let n = agents.len();
        self.token.reset(n);
        self.agent_map.clear();
        self.reverse_map.clear();

        let max_idx = agents.iter().map(|a| a.index).max().unwrap_or(0) + 1;
        self.agent_map.resize(max_idx, usize::MAX);
        for (local, a) in agents.iter().enumerate() {
            if a.index < self.agent_map.len() {
                self.agent_map[a.index] = local;
            }
            self.reverse_map.push(a.index);
        }

        for (local, a) in agents.iter().enumerate() {
            self.token.set_path(local, vec![a.pos]);
        }

        self.last_agent_indices.clear();
        self.last_agent_indices.extend(agents.iter().map(|a| a.index));

        self.initialized = true;
    }

    fn plan_for_agent(
        &mut self,
        local: usize,
        start: IVec2,
        goal: IVec2,
        grid: &GridMap,
        dist_cache: &mut DistanceMapCache,
    ) -> Option<Vec<IVec2>> {
        if start == goal {
            return Some(vec![start]);
        }

        // Master CI already has this agent's path removed by the caller.
        // REFERENCE: pibt2 tp.cpp:255-264 — last time another path uses `goal`.
        let after = self
            .token
            .paths
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != local)
            .filter_map(|(_, path)| path.iter().rposition(|&p| p == goal))
            .max()
            .map(|t| t as u64);
        let ci = ArriveAfter { inner: &self.master_ci, goal, after };

        let agent_pair = [(start, goal)];
        let maps = dist_cache.get_or_compute(grid, &agent_pair);
        let dm = maps[0];

        let actions = spacetime_astar_fast(
            grid,
            start,
            goal,
            &ci,
            TOKEN_ASTAR_MAX_TIME,
            Some(dm),
            &mut self.stg,
            ASTAR_MAX_EXPANSIONS,
            None,
        )
        .ok()?;

        let mut positions = Vec::with_capacity(actions.len() + 1);
        positions.push(start);
        let mut pos = start;
        for action in &actions {
            pos = action.apply(pos);
            positions.push(pos);
        }

        Some(positions)
    }

    /// pibt2 `updatePath2` (tp.cpp:206-248): nearest parking endpoint (BFS
    /// distance) that is walkable and not the end of another token path.
    /// `None` if every parking cell is taken (pibt2 halts: not well-formed).
    fn free_parking(&self, local: usize, pos: IVec2, ctx: &SolverContext) -> Option<IVec2> {
        let dm = DistanceMap::compute(ctx.grid, pos);
        ctx.zones
            .parking_cells()
            .into_iter()
            .filter(|&p| ctx.grid.is_walkable(p) && dm.get(p) != u64::MAX)
            .filter(|&p| {
                self.token
                    .paths
                    .iter()
                    .enumerate()
                    .all(|(j, path)| j == local || path.back() != Some(&p))
            })
            .min_by_key(|&p| dm.get(p))
    }
}

impl LifelongSolver for TokenPassingSolver {
    fn name(&self) -> &'static str {
        "token_passing"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n × A* per idle agent)",
            scalability: Scalability::Medium,
            description: "Token Passing — decentralized sequential planning. Each idle agent plans a collision-free path against all others' TOKEN paths.",
            source: "Ma et al., AAMAS 2017",
            // Instance-dependent: see `max_agents`.
            recommended_max_agents: None,
        }
    }

    fn max_agents(&self, zones: &crate::core::topology::ZoneMap) -> Option<usize> {
        Some(zones.parking_cells().len())
    }

    fn task_scheduler(&self) -> Option<&dyn crate::core::task::TaskScheduler> {
        Some(&crate::core::task::TokenPassingTaskSelection)
    }

    fn reset(&mut self) {
        self.token.reset(0);
        self.plan_buffer.clear();
        self.agent_map.clear();
        self.reverse_map.clear();
        self.last_agent_indices.clear();
        self.replanned_set.clear();
        self.goal_overrides.clear();
        self.initialized = false;
    }

    fn drain_goal_overrides(&mut self) -> Vec<(usize, IVec2)> {
        std::mem::take(&mut self.goal_overrides)
    }

    /// Restore TP's per-agent token paths from the snapshot's restored
    /// `planned_actions`. Called during rewind AFTER `reset()`. Without this,
    /// the master constraint index rebuilt on the next `step()` would be
    /// empty (every token length 1), which changes A* results and makes the
    /// rewound replay diverge from the original run.
    fn restore_state(&mut self, agents: &[AgentRestoreState]) {
        if agents.is_empty() {
            return;
        }

        // Re-initialise agent_map / reverse_map from the post-rewind agent
        // set. We fabricate the minimal `AgentState` slice that
        // `ensure_initialized` needs — `has_plan` / `task_leg` are ignored
        // there, only `index` and `pos` matter.
        let init_states: Vec<AgentState> = agents
            .iter()
            .map(|a| AgentState {
                index: a.index,
                pos: a.pos,
                goal: a.goal,
                has_plan: !a.planned_actions.is_empty(),
                task_leg: a.task_leg.clone(),
            })
            .collect();
        self.initialized = false;
        self.ensure_initialized(&init_states);

        // Walk each restored planned-action sequence to rebuild the position
        // stream of the token. The token stores the positions the agent IS
        // at now and will pass through in the next `planned_actions.len()`
        // ticks — exactly what `MasterConstraintIndex` consumes in the next
        // `step()` at line 257 to rebuild the shared vertex/edge blocks.
        for restore in agents {
            if let Some(&local) = self.agent_map.get(restore.index)
                && local < self.token.paths.len()
            {
                let mut positions = Vec::with_capacity(restore.planned_actions.len() + 1);
                positions.push(restore.pos);
                let mut p = restore.pos;
                for action in restore.planned_actions {
                    p = action.apply(p);
                    positions.push(p);
                }
                self.token.set_path(local, positions);
            }
        }
        // master_ci is rebuilt at the top of the next step() from these
        // restored tokens, so no explicit rebuild here.
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        _rng: &mut SeededRng,
    ) -> StepResult<'a> {
        if agents.is_empty() {
            self.plan_buffer.clear();
            return StepResult::Replan(&self.plan_buffer);
        }

        self.ensure_initialized(agents);
        self.plan_buffer.clear();
        self.goal_overrides.clear();

        // Advance token: pop the front of every path, simulating that all agents
        // moved to their TOKEN[i][1] position. This is the relative-time
        // equivalent of pibt2's `current_timestep++` (tp.cpp line 165 `P->update()`).
        self.token.advance();

        // Sync actual positions AND detect goal changes — MAFIS-specific
        // fault/queue handling. pibt2 has no equivalent because it's a
        // finite-task experiment with no fault model and no queue manager.
        //
        // Two invalidation triggers:
        //
        // 1. **Position drift**: agent didn't move as the token predicted
        //    (fault, latency injection, collision forced Wait, external
        //    position reset). Reset token to `[actual_pos]`.
        //
        // 2. **Goal drift**: agent's task leg now targets a different
        //    endpoint than the token's final cell. This happens when the
        //    queue manager kicks an agent from `Queuing` back to
        //    `Loading(pickup)` (queue full / queue reassignment), or when
        //    the task scheduler reassigns a pickup. Without this check, TP
        //    keeps executing the stale plan toward the old endpoint and
        //    agents appear "stuck" at the queue cell while displayed in the
        //    picking-state colour.
        for a in agents {
            if let Some(&local) = self.agent_map.get(a.index)
                && local < self.token.paths.len()
            {
                let path = &self.token.paths[local];
                let token_pos = path.front().copied();
                let token_end = path.back().copied();

                // Mirror `plan_for_agent` target-selection logic exactly.
                let current_target = match &a.task_leg {
                    TaskLeg::TravelEmpty(pickup) => Some(*pickup),
                    TaskLeg::TravelLoaded { to, .. } => Some(*to),
                    _ => a.goal,
                };
                let goal_stale =
                    path.len() > 1 && current_target.is_some() && token_end != current_target;

                if token_pos != Some(a.pos) || goal_stale {
                    self.token.set_path(local, vec![a.pos]);
                }
            }
        }

        // Build master constraint index from all token paths.
        // REFERENCE: pibt2 tp.cpp lines 274-296 `checkInvalidAstarNode` builds
        // a token_endpoints map and uses CONFLICT_TABLE to block cells. The
        // MasterConstraintIndex is the MAFIS equivalent: it adds vertex+edge
        // constraints for every cell in every TOKEN path, plus extends the
        // final cell of each path forever (the endpoint blocking).
        self.master_ci.reset(ctx.grid.width, ctx.grid.height, TOKEN_PATH_MAX_TIME);
        for path in &self.token.paths {
            self.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // Plan for agents that need replanning, in agent index order.
        // REFERENCE: pibt2 tp.cpp line 52 `for (auto a : A)` — iterates the
        // agent vector in insertion order. The sequential planning order is
        // semantically significant in TP because agent k's emitted plan
        // becomes a constraint for agent k+1.
        let mut planning_order: Vec<usize> = (0..agents.len()).collect();
        planning_order.sort_by_key(|&i| agents[i].index);

        for &agent_idx in &planning_order {
            let a = &agents[agent_idx];
            let local = match self.agent_map.get(a.index) {
                Some(&l) if l < self.token.paths.len() => l,
                _ => continue,
            };

            // REFERENCE: pibt2 tp.cpp line 53
            // `if ((int)TOKEN[a->id].size() - 1 != P->getCurrentTimestep())`
            // i.e. agent needs replan iff its token doesn't extend past now.
            // Relative-time equivalent: `paths[local].len() <= 1` means
            // path[0]=current_pos with no future planned steps.
            let token_idle = self.token.paths[local].len() <= 1;

            // REFERENCE: pibt2 tp.cpp:99-123. A free agent that got no task
            // stays put, unless it stands on a delivery endpoint: then it
            // moves to a parking endpoint (`updatePath2`).
            let parking = if token_idle
                && a.task_leg == TaskLeg::Free
                && a.goal == Some(a.pos)
                && ctx.zones.delivery_cells.contains(&a.pos)
            {
                self.free_parking(local, a.pos, ctx)
            } else {
                None
            };
            let needs_plan = token_idle && a.goal.is_some_and(|g| g != a.pos);

            if parking.is_none() && !needs_plan {
                continue;
            }

            let goal = parking.or(a.goal).unwrap_or(a.pos);

            // Remove this agent's own token from the constraint index before
            // planning (so the agent doesn't constrain itself).
            // REFERENCE: pibt2 tp.cpp line 278 `if (j == i) continue;` in the
            // token_endpoints loop, line 279 `if (j != i)` in the conflict-table
            // check — the agent's own path is excluded.
            let old_path = self.token.paths[local].clone();
            self.master_ci.remove_path(&old_path, TOKEN_PATH_MAX_TIME);

            // Plan with spacetime A* against the constraint index.
            // REFERENCE: pibt2 tp.cpp line 299 `getPathBySpaceTimeAstar(...)`.
            // The MAFIS adaptation routes through `task_leg` to pick the
            // current waypoint (pickup vs delivery vs goal), since MAFIS's
            // task scheduler manages the leg transitions externally.
            let path = match &a.task_leg {
                TaskLeg::TravelEmpty(pickup) => {
                    self.plan_for_agent(local, a.pos, *pickup, ctx.grid, distance_cache)
                }
                TaskLeg::TravelLoaded { from: _, to } => {
                    self.plan_for_agent(local, a.pos, *to, ctx.grid, distance_cache)
                }
                _ => self.plan_for_agent(local, a.pos, goal, ctx.grid, distance_cache),
            };

            if let Some(positions) = path {
                if let Some(p) = parking {
                    self.goal_overrides.push((a.index, p));
                }
                let actions: smallvec::SmallVec<[Action; 20]> = positions
                    .windows(2)
                    .map(|w| crate::solver::shared::heuristics::delta_to_action(w[0], w[1]))
                    .collect();

                if !actions.is_empty() {
                    self.plan_buffer.push((a.index, actions));
                }

                // Update token and add new path to master index
                let new_path: VecDeque<IVec2> = positions.iter().copied().collect();
                self.master_ci.add_path(&new_path, TOKEN_PATH_MAX_TIME);
                self.token.set_path(local, positions);
            } else {
                // Planning failed — re-add old path
                self.master_ci.add_path(&old_path, TOKEN_PATH_MAX_TIME);
            }
        }

        // For agents with existing paths (not replanned), emit their next action
        self.replanned_set.clear();
        self.replanned_set.extend(self.plan_buffer.iter().map(|(idx, _)| *idx));

        for a in agents {
            if self.replanned_set.contains(&a.index) {
                continue;
            }

            let local = match self.agent_map.get(a.index) {
                Some(&l) if l < self.token.paths.len() => l,
                _ => continue,
            };

            let path = &self.token.paths[local];
            if path.len() >= 2 {
                let action = crate::solver::shared::heuristics::delta_to_action(path[0], path[1]);
                self.plan_buffer.push((a.index, smallvec![action]));
            } else {
                self.plan_buffer.push((a.index, smallvec![Action::Wait]));
            }
        }

        StepResult::Replan(&self.plan_buffer)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::topology::ZoneMap;
    use crate::solver::shared::heuristics::DistanceMapCache;
    use std::collections::HashMap;

    fn test_zones() -> ZoneMap {
        ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(4, 4)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        }
    }

    #[test]
    fn tp_empty_agents() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 0 };
        let result = solver.step(&ctx, &[], &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(plans) if plans.is_empty()));
    }

    #[test]
    fn tp_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let mut pos = IVec2::ZERO;
        let goal = IVec2::new(4, 4);

        for tick in 0..20 {
            let agents = vec![AgentState {
                index: 0,
                pos,
                goal: Some(goal),
                has_plan: tick > 0,
                task_leg: TaskLeg::TravelEmpty(goal),
            }];

            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 1 };
            match solver.step(&ctx, &agents, &mut cache, &mut rng) {
                StepResult::Replan(plans) => {
                    if let Some((_, actions)) = plans.first() {
                        if let Some(action) = actions.first() {
                            let new_pos = action.apply(pos);
                            assert!(grid.is_walkable(new_pos), "moved to obstacle at tick {tick}");
                            pos = new_pos;
                        }
                    }
                }
                StepResult::Continue => {}
            }

            if pos == goal {
                return;
            }
        }

        assert_eq!(pos, goal, "agent should reach goal within 20 ticks");
    }

    #[test]
    fn tp_two_agents_no_collision() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let mut positions = [IVec2::new(0, 2), IVec2::new(4, 2)];
        let goals = [IVec2::new(4, 2), IVec2::new(0, 2)];

        for tick in 0..30 {
            let agents: Vec<AgentState> = (0..2)
                .map(|i| AgentState {
                    index: i,
                    pos: positions[i],
                    goal: Some(goals[i]),
                    has_plan: tick > 0,
                    task_leg: TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();

            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 2 };
            match solver.step(&ctx, &agents, &mut cache, &mut rng) {
                StepResult::Replan(plans) => {
                    for (idx, actions) in plans {
                        if let Some(action) = actions.first() {
                            let new_pos = action.apply(positions[*idx]);
                            assert!(grid.is_walkable(new_pos));
                            positions[*idx] = new_pos;
                        }
                    }
                }
                StepResult::Continue => {}
            }

            if positions[0] == positions[1] {
                panic!("vertex collision at tick {tick}: {:?}", positions);
            }
        }
    }

    fn tp_agent(index: usize, pos: IVec2, goal: IVec2, task_leg: TaskLeg) -> AgentState {
        AgentState { index, pos, goal: Some(goal), has_plan: false, task_leg }
    }

    /// pibt2 tp.cpp:255-272 (`checkAstarFin`): an agent may only finish at its
    /// goal after the last time another token path passes through that cell,
    /// otherwise it would rest on that path.
    #[test]
    fn tp_arrives_after_other_paths_leave_goal() {
        let grid = GridMap::new(5, 3);
        let zones = test_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let goal = IVec2::new(2, 1);
        // Agent 0 plans first and crosses `goal` at t=2 on its way to (4,1).
        let agents = vec![
            tp_agent(0, IVec2::new(0, 1), IVec2::new(4, 1), TaskLeg::TravelEmpty(IVec2::new(4, 1))),
            tp_agent(1, IVec2::new(2, 0), goal, TaskLeg::TravelEmpty(goal)),
        ];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };
        let _ = solver.step(&ctx, &agents, &mut cache, &mut rng);
        let crossing = solver.token.paths[0].iter().position(|&p| p == goal).unwrap();
        let arrival = solver.token.paths[1].iter().position(|&p| p == goal).unwrap();
        assert_eq!(solver.token.paths[1].back(), Some(&goal));
        assert!(arrival > crossing, "arrived at t={arrival}, other path crosses at t={crossing}");
    }

    fn parking_zones() -> ZoneMap {
        let mut zones = test_zones();
        zones.delivery_cells = vec![IVec2::new(4, 0)];
        zones.zone_type.insert(IVec2::new(0, 2), crate::core::topology::ZoneType::Parking);
        zones.zone_type.insert(IVec2::new(4, 2), crate::core::topology::ZoneType::Parking);
        zones
    }

    /// pibt2 tp.cpp:99-123 (`updatePath2`): a free agent with no task that
    /// stands on a delivery endpoint moves to the nearest parking endpoint
    /// that is not the end of another token path. A free agent elsewhere stays.
    #[test]
    fn tp_free_agent_on_delivery_moves_to_free_parking() {
        let grid = GridMap::new(5, 3);
        let zones = parking_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let dock = IVec2::new(4, 0);
        let rest = IVec2::new(4, 2); // nearest parking, but agent 1 rests there
        let agents =
            vec![tp_agent(0, dock, dock, TaskLeg::Free), tp_agent(1, rest, rest, TaskLeg::Free)];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };
        let _ = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert_eq!(solver.drain_goal_overrides(), vec![(0, IVec2::new(0, 2))]);
        assert_eq!(solver.token.paths[0].back(), Some(&IVec2::new(0, 2)));
        assert_eq!(solver.token.paths[1].iter().copied().collect::<Vec<_>>(), vec![rest]);
    }

    /// A parking cell blocked by a dead agent (now an obstacle) is skipped.
    #[test]
    fn tp_parking_skips_blocked_cells() {
        let mut grid = GridMap::new(5, 3);
        grid.set_obstacle(IVec2::new(4, 2));
        let zones = parking_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let dock = IVec2::new(4, 0);
        let agents = vec![tp_agent(0, dock, dock, TaskLeg::Free)];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let _ = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert_eq!(solver.drain_goal_overrides(), vec![(0, IVec2::new(0, 2))]);
    }

    /// 10×7 well-formed map: 4 parking (row 1), 3 pickups (row 3), one dock
    /// at (8,5) with a 4-cell queue extending west.
    fn tiny_well_formed_map() -> (GridMap, ZoneMap) {
        let mut cells = Vec::new();
        for x in 0..10 {
            cells.push(serde_json::json!({"x": x, "y": 0, "type": "wall"}));
            cells.push(serde_json::json!({"x": x, "y": 6, "type": "wall"}));
        }
        for y in 1..6 {
            cells.push(serde_json::json!({"x": 0, "y": y, "type": "wall"}));
            cells.push(serde_json::json!({"x": 9, "y": y, "type": "wall"}));
        }
        for x in [1, 3, 5, 7] {
            cells.push(serde_json::json!({"x": x, "y": 1, "type": "parking"}));
        }
        for x in [2, 4, 6] {
            cells.push(serde_json::json!({"x": x, "y": 3, "type": "pickup"}));
        }
        cells.push(
            serde_json::json!({"x": 8, "y": 5, "type": "delivery", "queue_direction": "west"}),
        );
        let json = serde_json::json!({"width": 10, "height": 7, "cells": cells});
        crate::core::topology::TopologyRegistry::parse_json_value(&json).unwrap()
    }

    /// Runs TP through the full runner (task system + queue) and returns the
    /// per-tick positions plus the tick of every completed task.
    fn run_tp(seed: u64, num_agents: usize, ticks: u64) -> (Vec<Vec<IVec2>>, Vec<u64>) {
        use crate::core::queue::ActiveQueuePolicy;
        use crate::core::runner::SimulationRunner;
        use crate::core::task::ClosestFirstScheduler;
        use crate::fault::config::FaultConfig;
        use crate::fault::scenario::FaultSchedule;

        let (grid, zones) = tiny_well_formed_map();
        let wf = crate::core::topology::check_well_formed(&grid, &zones);
        assert!(wf.holds_for(num_agents), "{wf:?}");
        let mut rng = SeededRng::new(seed);
        let agents = crate::analysis::baseline::place_agents(num_agents, &grid, &zones, &mut rng);
        let mut runner = SimulationRunner::new(
            grid,
            zones.clone(),
            agents,
            Box::new(TokenPassingSolver::new()),
            rng,
            FaultConfig { enabled: false, ..Default::default() },
            FaultSchedule::default(),
        );
        let policy = ActiveQueuePolicy::from_name("closest");
        let mut trace = Vec::new();
        let mut completions = Vec::new();
        for _ in 0..ticks {
            let result = runner.tick(&ClosestFirstScheduler, policy.policy());
            completions.extend(result.completion_ticks.iter().copied());
            // Endpoint rule: no two path ends coincide on pickups or parking.
            // Queue-line and dock cells are assigned by the shared queue
            // manager, which can send two agents to the same slot or hand a
            // dock over while its last user still stands on it (deviation).
            let queue_cell = |c: IVec2| {
                zones.queue_lines.iter().any(|l| l.delivery_cell == c || l.cells.contains(&c))
            };
            let alive: Vec<_> = runner.agents.iter().filter(|a| a.alive).collect();
            for (i, a) in alive.iter().enumerate() {
                for b in &alive[i + 1..] {
                    assert!(
                        a.goal != b.goal || queue_cell(a.goal),
                        "tick {}: two paths end at {} ({:?} at {}, {:?} at {})",
                        result.tick,
                        a.goal,
                        a.task_leg,
                        a.pos,
                        b.task_leg,
                        b.pos
                    );
                }
            }
            trace.push(runner.agents.iter().map(|a| a.pos).collect());
        }
        (trace, completions)
    }

    /// On a well-formed instance TP keeps completing tasks (no deadlock).
    #[test]
    fn tp_well_formed_map_completes_tasks_without_deadlock() {
        for seed in 1..=10 {
            let (_, completions) = run_tp(seed, 4, 600);
            assert!(completions.len() >= 20, "seed {seed}: only {} tasks", completions.len());
            let longest_gap = completions.windows(2).map(|w| w[1] - w[0]).max().unwrap();
            assert!(longest_gap <= 60, "seed {seed}: no completion for {longest_gap} ticks");
        }
    }

    /// Same seed → identical trajectories.
    #[test]
    fn tp_is_deterministic() {
        assert_eq!(run_tp(7, 4, 300), run_tp(7, 4, 300));
    }

    /// TP's fleet cap comes from the instance: Ma et al. well-formedness
    /// condition 2 needs at least one non-task endpoint (parking) per agent.
    #[test]
    fn tp_max_agents_is_parking_count() {
        use crate::core::topology::TopologyRegistry;
        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let zones_of =
            |id: &str| TopologyRegistry::parse_entry(registry.find(id).unwrap()).unwrap().1;
        let tp = TokenPassingSolver::new();
        assert_eq!(tp.max_agents(&zones_of("warehouse_single_dock")), Some(60));
        assert_eq!(tp.max_agents(&zones_of("warehouse_dual_dock")), Some(120));
        let pibt = crate::solver::pibt::PibtLifelongSolver::new();
        assert_eq!(pibt.max_agents(&zones_of("warehouse_single_dock")), None);
    }

    /// TP selects its own tasks (Ma et al. 2017 Alg. 1 lines 7-12) instead of
    /// taking them from the configured scheduler.
    #[test]
    fn tp_provides_its_own_task_selection() {
        let solver = TokenPassingSolver::new();
        assert_eq!(solver.task_scheduler().map(|s| s.name()), Some("token_passing"));
        let pibt = crate::solver::pibt::PibtLifelongSolver::new();
        assert!(pibt.task_scheduler().is_none());
    }

    #[test]
    fn tp_reset_clears_state() {
        let mut solver = TokenPassingSolver::new();
        solver.reset();
        assert!(!solver.initialized);
        assert!(solver.token.paths.is_empty());
    }

    /// Regression for the 2026-04-20 goal-change-sync bug (Issue 1 of the
    /// TP audit). When an agent's `task_leg` changes to a new target (e.g.
    /// queue manager kicks `Queuing` back to `Loading(pickup)`), the token's
    /// stale endpoint must trigger a token reset so the agent replans toward
    /// the new goal instead of continuing along the old plan.
    #[test]
    fn tp_goal_change_resets_token() {
        let grid = GridMap::new(8, 3);
        let zones = test_zones();
        let mut solver = TokenPassingSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        // Tick 0: agent at (0,1) planning toward far goal (7,1).
        let far_goal = IVec2::new(7, 1);
        let near_goal = IVec2::new(2, 1);
        let start = IVec2::new(0, 1);

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let agents_far = vec![AgentState {
            index: 0,
            pos: start,
            goal: Some(far_goal),
            has_plan: false,
            task_leg: TaskLeg::TravelEmpty(far_goal),
        }];
        let _ = solver.step(&ctx, &agents_far, &mut cache, &mut rng);
        let token_end_after_first_step = solver.token.paths[0].back().copied();
        assert_eq!(
            token_end_after_first_step,
            Some(far_goal),
            "token should end at the planned goal after the first step",
        );
        assert!(
            solver.token.paths[0].len() > 1,
            "token should hold a multi-step plan to the far goal",
        );

        // Tick 1: same agent, same position — but target has changed from
        // far_goal to near_goal (analogous to the queue-kick case).
        let ctx1 = SolverContext { grid: &grid, zones: &zones, tick: 1, num_agents: 1 };
        let agents_near = vec![AgentState {
            index: 0,
            pos: start,
            goal: Some(near_goal),
            has_plan: true,
            task_leg: TaskLeg::TravelEmpty(near_goal),
        }];
        let _ = solver.step(&ctx1, &agents_near, &mut cache, &mut rng);
        let token_end_after_second_step = solver.token.paths[0].back().copied();
        assert_eq!(
            token_end_after_second_step,
            Some(near_goal),
            "token must re-plan to the new target after a goal change, not continue the stale plan",
        );
    }

    /// Regression for the 2026-04-20 rewind-determinism bug (Issue 3 of the
    /// TP audit). After `restore_state`, each agent's token must equal the
    /// position sequence derived from walking its restored `planned_actions`
    /// from `pos`, so the `MasterConstraintIndex` rebuilt on the next
    /// `step()` matches the original run bit-for-bit.
    #[test]
    fn tp_restore_state_rebuilds_token_from_actions() {
        use crate::core::action::{Action, Direction};
        use crate::solver::lifelong::AgentRestoreState;

        let mut solver = TokenPassingSolver::new();
        solver.reset(); // starts uninitialised

        let actions_0 = vec![
            Action::Move(Direction::East),
            Action::Move(Direction::East),
            Action::Move(Direction::East),
        ];
        let actions_1 = vec![Action::Move(Direction::North), Action::Move(Direction::North)];

        let restore = vec![
            AgentRestoreState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(3, 0)),
                task_leg: TaskLeg::TravelEmpty(IVec2::new(3, 0)),
                planned_actions: &actions_0,
            },
            AgentRestoreState {
                index: 1,
                pos: IVec2::new(5, 0),
                goal: Some(IVec2::new(5, 2)),
                task_leg: TaskLeg::TravelEmpty(IVec2::new(5, 2)),
                planned_actions: &actions_1,
            },
        ];

        solver.restore_state(&restore);

        // Tokens should trace [start, after_action_0, after_action_1, ...].
        let token_0: Vec<IVec2> = solver.token.paths[0].iter().copied().collect();
        assert_eq!(
            token_0,
            vec![IVec2::new(0, 0), IVec2::new(1, 0), IVec2::new(2, 0), IVec2::new(3, 0),],
        );
        let token_1: Vec<IVec2> = solver.token.paths[1].iter().copied().collect();
        assert_eq!(token_1, vec![IVec2::new(5, 0), IVec2::new(5, 1), IVec2::new(5, 2)],);
    }

    /// Empty restore is a no-op (exercises the early-return branch).
    #[test]
    fn tp_restore_state_empty_is_noop() {
        use crate::solver::lifelong::AgentRestoreState;
        let mut solver = TokenPassingSolver::new();
        solver.restore_state(&[] as &[AgentRestoreState]);
        assert!(solver.token.paths.is_empty(), "empty restore must not touch tokens");
        assert!(!solver.initialized);
    }

    /// Regression test for the Token Passing audit (Step 3 of solver-refocus).
    ///
    /// Locks the current MAFIS TP throughput on a known instance so that the
    /// 2026-04-07 fix to planning order (tasked-first → agent-index order to
    /// match pibt2 line 52) doesn't drift in future changes. Floor is set to
    /// 50% of the measured baseline to allow noise but catch real regressions.
    ///
    /// REFERENCE: docs/papers_codes/pibt/pibt2/src/tp.cpp (Okumura, pibt2).
    #[test]
    fn token_passing_throughput_regression() {
        use crate::core::topology::TopologyRegistry;
        use crate::experiment::config::ExperimentConfig;
        use crate::experiment::runner::run_single_experiment;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        assert!(
            registry.find("warehouse_single_dock").is_some(),
            "warehouse_single_dock.json missing"
        );

        let config = ExperimentConfig {
            solver_name: "token_passing".into(),
            topology_name: "warehouse_single_dock".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 20,
            seed: 42,
            tick_count: 200,
            custom_map: None,
            rhcr_override: None,
        };
        let result = run_single_experiment(&config);
        let tp = result.baseline_metrics.avg_throughput;
        eprintln!("token_passing_throughput_regression: tp={tp:.4} tasks/tick");
        // Token Passing on warehouse_single_dock at 20 agents should produce
        // measurable throughput (>0.05 tasks/tick). A regression to near-zero
        // would indicate broken sequential planning or constraint logic.
        assert!(
            tp > 0.05,
            "Token Passing regression: avg_throughput {tp:.4} fell below 0.05 floor. \
             Check planning order, MasterConstraintIndex add/remove symmetry, and \
             pibt2 line 53 needs-replan condition."
        );
    }
}
