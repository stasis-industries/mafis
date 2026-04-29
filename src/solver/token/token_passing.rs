//! Token Passing — decentralized lifelong MAPF solver.
//!
//! REFERENCE: docs/papers_codes/pibt/pibt2/src/tp.cpp (Okumura et al., pibt2)
//!            docs/papers_codes/pibt/pibt2/include/tp.hpp
//! Paper: Ma, Tovey, Sharon, Kumar, Koenig — "Lifelong Multi-Agent Path Finding
//! for Online Pickup and Delivery Tasks", AAMAS 2017.
//!
//! Audited 2026-04-07 against the canonical pibt2 reference (~320 lines C++).
//!
//! ## Structural mapping (MAFIS ↔ pibt2)
//!
//! The MAFIS implementation is **semantically equivalent** to pibt2's canonical
//! algorithm but expressed in different but isomorphic data structures. Specifically:
//!
//! | pibt2 (absolute time)              | MAFIS (relative time)                  |
//! |------------------------------------|----------------------------------------|
//! | `TOKEN[i]: vector<Node>` grows     | `Token::paths[i]: VecDeque<IVec2>`     |
//! | `current_timestep` increments      | `Token::advance()` pops front each tick|
//! | `TOKEN[i][current_t]` = current pos| `paths[i].front()` = current pos       |
//! | `TOKEN[i].size()-1 == current_t`   | `paths[i].len() <= 1` (no future plan) |
//! | `CONFLICT_TABLE[t][cell] != NIL`   | `MasterConstraintIndex::is_vertex_blocked(cell, t)` |
//! | endpoint blocking via `token_endpoints[v]` + `TOKEN[k].size()-1 < t` | `MasterConstraintIndex::add_path` extends final-cell vertex constraints to `max_time` |
//! | `checkAstarFin: g + current_t > max_constraint_time` | vertex constraints on goal cell at all blocked times naturally force A* to delay arrival |
//!
//! ## Adaptations to MAFIS architecture
//!
//! MAFIS separates task scheduling from path planning. Where pibt2 picks tasks
//! inside the solver loop (`tp.cpp` lines 64-103), MAFIS receives `agent.goal:
//! Option<IVec2>` already set by the `TaskScheduler` trait (see
//! `src/core/task/`). Therefore the pibt2 task assignment phase is omitted —
//! MAFIS only ports the **path planning portion** of TP (`tp.cpp` lines 250-317
//! `updatePath` + the conflict-table A* hooks).
//!
//! Position synchronization (`step()` lines 187-196) is a MAFIS-specific addition
//! that handles fault-induced divergence: if an agent's actual position differs
//! from its expected token position, the token is reset to `[actual_pos]` and
//! the agent is replanned next tick. pibt2 has no fault handling because it's
//! a finite-task experiment.
//!
//! ## Tracked deviations from the canonical algorithm
//!
//! 1. **Relative vs absolute time** — semantically equivalent (see mapping table
//!    above). Relative time gives bounded memory for lifelong operation; absolute
//!    time would grow `TOKEN[i]` indefinitely.
//!
//! 2. **No `updatePath2` (closest non-conflicting endpoint selection)** — pibt2
//!    line 206-248 handles the case where an agent has no task and its current
//!    cell is a delivery location, by moving to the closest free endpoint.
//!    MAFIS's task scheduler handles idle-state task generation differently
//!    (the `TaskLeg::Free` state machine), so this branch isn't applicable.
//!
//! 3. **Planning order: pibt2 line 52 iterates agents in vector order**. The
//!    sequential planning order matters in TP because agent k's plan affects
//!    agent k+1's constraints. MAFIS now matches this (was previously
//!    "tasked-first" — fixed in 2026-04-07 audit).

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
use crate::solver::shared::astar::{SpacetimeGrid, spacetime_astar_fast};
use crate::solver::shared::heuristics::DistanceMapCache;
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
        _local: usize,
        start: IVec2,
        goal: IVec2,
        grid: &GridMap,
        dist_cache: &mut DistanceMapCache,
    ) -> Option<Vec<IVec2>> {
        if start == goal {
            return Some(vec![start]);
        }

        // Master CI already has this agent's path removed by the caller.
        // Use it directly as the constraint source.

        let agent_pair = [(start, goal)];
        let maps = dist_cache.get_or_compute(grid, &agent_pair);
        let dm = maps[0];

        let actions = spacetime_astar_fast(
            grid,
            start,
            goal,
            &self.master_ci,
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
            recommended_max_agents: Some(100),
        }
    }

    fn reset(&mut self) {
        self.token.reset(0);
        self.plan_buffer.clear();
        self.agent_map.clear();
        self.reverse_map.clear();
        self.last_agent_indices.clear();
        self.replanned_set.clear();
        self.initialized = false;
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
            let needs_plan =
                self.token.paths[local].len() <= 1 && a.goal.is_some() && a.pos != a.goal.unwrap();

            if !needs_plan {
                continue;
            }

            let goal = a.goal.unwrap();

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
