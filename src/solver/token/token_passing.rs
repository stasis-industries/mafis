//! Token Passing — decentralized lifelong MAPF solver.
//!
//! Each tick, idle agents plan collision-free paths by treating all other agents'
//! planned paths as moving obstacles (via the shared TOKEN). One agent plans at
//! a time (sequentially), using spacetime A* with constraints built from
//! the TOKEN.
//!
//! Reference: Ma et al., "Lifelong Multi-Agent Path Finding for Online Pickup
//! and Delivery Tasks" (AAMAS 2017).

use bevy::prelude::*;
use smallvec::smallvec;
use std::collections::VecDeque;

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;

use super::common::{MasterConstraintIndex, Token};
use crate::solver::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
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

        // Advance token
        self.token.advance();

        // Sync actual positions
        for a in agents {
            if let Some(&local) = self.agent_map.get(a.index)
                && local < self.token.paths.len()
            {
                let token_pos = self.token.paths[local].front().copied();
                if token_pos != Some(a.pos) {
                    self.token.set_path(local, vec![a.pos]);
                }
            }
        }

        // Build master constraint index once from all token paths
        self.master_ci.reset(ctx.grid.width, ctx.grid.height, TOKEN_PATH_MAX_TIME);
        for path in &self.token.paths {
            self.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // Plan for agents that need replanning
        let mut planning_order: Vec<usize> = (0..agents.len()).collect();
        planning_order.sort_by_key(|&i| {
            let has_task = !matches!(agents[i].task_leg, TaskLeg::Free);
            (!has_task, agents[i].index)
        });

        for &agent_idx in &planning_order {
            let a = &agents[agent_idx];
            let local = match self.agent_map.get(a.index) {
                Some(&l) if l < self.token.paths.len() => l,
                _ => continue,
            };

            let needs_plan =
                self.token.paths[local].len() <= 1 && a.goal.is_some() && a.pos != a.goal.unwrap();

            if !needs_plan {
                continue;
            }

            let goal = a.goal.unwrap();

            // Remove this agent's path from the master index
            let old_path = self.token.paths[local].clone();
            self.master_ci.remove_path(&old_path, TOKEN_PATH_MAX_TIME);

            // Plan using the master index (agent's own path excluded)
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

        let mut positions = vec![IVec2::new(0, 2), IVec2::new(4, 2)];
        let goals = vec![IVec2::new(4, 2), IVec2::new(0, 2)];

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
}
