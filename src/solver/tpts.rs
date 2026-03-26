//! TPTS — Token Passing with Task Swaps.
//!
//! Extends Token Passing with goal swapping: after standard planning,
//! pairs of nearby agents swap goals when it reduces total path cost.
//! Swap cooldown and TaskLeg compatibility checks prevent oscillation.
//!
//! Reference: Ma et al., "Lifelong Multi-Agent Path Finding for Online
//! Pickup and Delivery Tasks" (AAMAS 2017).

use bevy::prelude::*;
use smallvec::smallvec;
use std::collections::{HashMap, VecDeque};

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;

use super::astar::{SpacetimeGrid, spacetime_astar_fast};
use super::heuristics::{manhattan, DistanceMapCache};
use super::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use super::token_common::{MasterConstraintIndex, Token};
use super::traits::{Optimality, Scalability, SolverInfo};

use crate::constants::{
    ASTAR_MAX_EXPANSIONS, TOKEN_ASTAR_MAX_TIME, TOKEN_PATH_MAX_TIME, TPTS_MAX_SWAP_CHECKS,
    TPTS_SWAP_COOLDOWN, TPTS_SWAP_RADIUS,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Two agents can only swap if their task legs are compatible.
fn legs_compatible(a: &TaskLeg, b: &TaskLeg) -> bool {
    matches!(
        (a, b),
        (TaskLeg::TravelEmpty(_), TaskLeg::TravelEmpty(_))
            | (TaskLeg::TravelLoaded { .. }, TaskLeg::TravelLoaded { .. })
    )
}

/// Canonical pair key — always (min, max) so (a,b) == (b,a).
fn pair_key(a: usize, b: usize) -> (usize, usize) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

// ---------------------------------------------------------------------------
// TPTS Solver
// ---------------------------------------------------------------------------

pub struct TptsSolver {
    token: Token,
    plan_buffer: Vec<AgentPlan>,
    agent_map: Vec<usize>,
    reverse_map: Vec<usize>,
    last_agent_indices: Vec<usize>,
    initialized: bool,
    master_ci: MasterConstraintIndex,
    stg: SpacetimeGrid,
    replanned_set: std::collections::HashSet<usize>,

    // TPTS extension
    max_swap_checks: usize,
    swap_radius: i32,
    swap_cooldown: HashMap<(usize, usize), u64>,
}

impl Default for TptsSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TptsSolver {
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
            max_swap_checks: TPTS_MAX_SWAP_CHECKS,
            swap_radius: TPTS_SWAP_RADIUS,
            swap_cooldown: HashMap::new(),
        }
    }

    fn ensure_initialized(&mut self, agents: &[AgentState]) {
        let same_set = self.initialized
            && self.last_agent_indices.len() == agents.len()
            && self
                .last_agent_indices
                .iter()
                .zip(agents.iter())
                .all(|(&a, b)| a == b.index);
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
        self.last_agent_indices
            .extend(agents.iter().map(|a| a.index));
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

    /// Find beneficial swaps: pairs where swapping goals reduces total Manhattan cost.
    /// Filters by: compatible task legs, within radius, not in cooldown.
    fn find_swaps(&mut self, agents: &[AgentState], tick: u64) -> Vec<(usize, usize)> {
        // Clean up stale cooldown entries
        self.swap_cooldown
            .retain(|_, cooldown_tick| tick < *cooldown_tick + TPTS_SWAP_COOLDOWN);

        let mut swaps = Vec::new();
        let mut checks = 0;

        for i in 0..agents.len() {
            if checks >= self.max_swap_checks {
                break;
            }

            let goal_i = match agents[i].goal {
                Some(g) if g != agents[i].pos => g,
                _ => continue,
            };

            for j in (i + 1)..agents.len() {
                if checks >= self.max_swap_checks {
                    break;
                }

                let goal_j = match agents[j].goal {
                    Some(g) if g != agents[j].pos => g,
                    _ => continue,
                };

                // Task leg compatibility
                if !legs_compatible(&agents[i].task_leg, &agents[j].task_leg) {
                    continue;
                }

                // Distance filter
                if manhattan(agents[i].pos, agents[j].pos) > self.swap_radius as u64 {
                    continue;
                }

                // Cooldown check
                let key = pair_key(agents[i].index, agents[j].index);
                if self.swap_cooldown.contains_key(&key) {
                    continue;
                }

                checks += 1;

                // Cost comparison
                let cost_current =
                    manhattan(agents[i].pos, goal_i) + manhattan(agents[j].pos, goal_j);
                let cost_swapped =
                    manhattan(agents[i].pos, goal_j) + manhattan(agents[j].pos, goal_i);

                if cost_swapped < cost_current {
                    swaps.push((i, j));
                    // Record cooldown
                    self.swap_cooldown.insert(key, tick);
                }
            }
        }

        swaps
    }
}

impl LifelongSolver for TptsSolver {
    fn name(&self) -> &'static str {
        "tpts"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n × A* + swap_checks) per replan",
            scalability: Scalability::Medium,
            description:
                "TPTS — Token Passing with Task Swaps. Decentralized sequential planning with goal swapping.",
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
        self.swap_cooldown.clear();
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

        // Build master constraint index
        self.master_ci
            .reset(ctx.grid.width, ctx.grid.height, TOKEN_PATH_MAX_TIME);
        for path in &self.token.paths {
            self.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // TPTS extension: find beneficial swaps
        let swaps = self.find_swaps(agents, ctx.tick);

        // Build swapped goals map
        let mut swapped_goals: Vec<Option<IVec2>> = agents.iter().map(|a| a.goal).collect();
        for &(i, j) in &swaps {
            let tmp = swapped_goals[i];
            swapped_goals[i] = swapped_goals[j];
            swapped_goals[j] = tmp;
        }

        // Plan for agents that need replanning (using potentially swapped goals)
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

            let goal = swapped_goals[agent_idx];
            let needs_plan = self.token.paths[local].len() <= 1
                && goal.is_some()
                && a.pos != goal.unwrap();

            if !needs_plan {
                continue;
            }

            let goal = goal.unwrap();

            // Remove this agent's path from master index
            let old_path = self.token.paths[local].clone();
            self.master_ci.remove_path(&old_path, TOKEN_PATH_MAX_TIME);

            let path = self.plan_for_agent(local, a.pos, goal, ctx.grid, distance_cache);

            if let Some(positions) = path {
                let actions: smallvec::SmallVec<[Action; 20]> = positions
                    .windows(2)
                    .map(|w| super::heuristics::delta_to_action(w[0], w[1]))
                    .collect();

                if !actions.is_empty() {
                    self.plan_buffer.push((a.index, actions));
                }

                let new_path: VecDeque<IVec2> = positions.iter().copied().collect();
                self.master_ci.add_path(&new_path, TOKEN_PATH_MAX_TIME);
                self.token.set_path(local, positions);
            } else {
                self.master_ci.add_path(&old_path, TOKEN_PATH_MAX_TIME);
            }
        }

        // Emit next actions for agents with existing paths
        self.replanned_set.clear();
        self.replanned_set
            .extend(self.plan_buffer.iter().map(|(idx, _)| *idx));

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
                let action = super::heuristics::delta_to_action(path[0], path[1]);
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
    use crate::solver::heuristics::DistanceMapCache;
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
    fn tpts_empty_agents() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let ctx = SolverContext {
            grid: &grid,
            zones: &zones,
            tick: 0,
            num_agents: 0,
        };
        let result = solver.step(&ctx, &[], &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(plans) if plans.is_empty()));
    }

    #[test]
    fn tpts_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TptsSolver::new();
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
            let ctx = SolverContext {
                grid: &grid,
                zones: &zones,
                tick,
                num_agents: 1,
            };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                if let Some((_, actions)) = plans.first() {
                    if let Some(action) = actions.first() {
                        pos = action.apply(pos);
                    }
                }
            }
            if pos == goal {
                return;
            }
        }
        assert_eq!(pos, goal);
    }

    #[test]
    fn tpts_swap_beneficial() {
        let mut solver = TptsSolver::new();
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(4, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1,
                pos: IVec2::new(3, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];
        // Current: 0→(4,0) costs 4, 1→(1,0) costs 2. Total = 6.
        // Swapped: 0→(1,0) costs 1, 1→(4,0) costs 1. Total = 2.
        let swaps = solver.find_swaps(&agents, 0);
        assert!(!swaps.is_empty());
        assert_eq!(swaps[0], (0, 1));
    }

    #[test]
    fn tpts_no_swap_incompatible_legs() {
        let mut solver = TptsSolver::new();
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(4, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1,
                pos: IVec2::new(3, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: false,
                task_leg: TaskLeg::Free, // Incompatible with TravelEmpty
            },
        ];
        let swaps = solver.find_swaps(&agents, 0);
        assert!(swaps.is_empty());
    }

    #[test]
    fn tpts_swap_cooldown() {
        let mut solver = TptsSolver::new();
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(4, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1,
                pos: IVec2::new(3, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];

        // First call: swap found
        let swaps = solver.find_swaps(&agents, 0);
        assert!(!swaps.is_empty());

        // Immediately after: cooldown prevents re-swap
        let swaps = solver.find_swaps(&agents, 1);
        assert!(swaps.is_empty(), "cooldown should prevent re-swap");

        // After cooldown expires
        let swaps = solver.find_swaps(&agents, TPTS_SWAP_COOLDOWN + 1);
        assert!(!swaps.is_empty(), "swap should be allowed after cooldown");
    }

    #[test]
    fn tpts_reset_clears_state() {
        let mut solver = TptsSolver::new();
        solver.swap_cooldown.insert((0, 1), 42);
        solver.reset();
        assert!(!solver.initialized);
        assert!(solver.token.paths.is_empty());
        assert!(solver.swap_cooldown.is_empty());
    }

    #[test]
    fn tpts_two_agents_no_collision() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TptsSolver::new();
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

            let ctx = SolverContext {
                grid: &grid,
                zones: &zones,
                tick,
                num_agents: 2,
            };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        positions[*idx] = action.apply(positions[*idx]);
                    }
                }
            }
            assert_ne!(
                positions[0], positions[1],
                "vertex collision at tick {tick}"
            );
        }
    }
}
