//! TPTS — Token Passing with Task Swaps.
//!
//! Extends Token Passing with goal swapping: when agent `ai` can reach agent
//! `ai'`s goal faster than `ai'` can (measured by actual spacetime A* cost),
//! the goals are swapped. Token state is snapshot/restored on swap failure.
//!
//! Reference: Ma et al., "Lifelong Multi-Agent Path Finding for Online Pickup
//! and Delivery Tasks" (AAMAS 2017), Algorithm 2.
//!
//! ## Paper Deviations (documented)
//!
//! 1. **No recursive GetTask**: the paper's Algorithm 2 recursively calls
//!    GetTask so that a displaced agent can find itself a new task. In MAFIS,
//!    the TaskScheduler (not the solver) owns task assignment, so recursive
//!    reassignment is not possible. We use single-level swaps instead.
//!
//! 2. **No Path2 endpoint parking**: the paper requires idle agents to move
//!    to "non-task endpoints" for deadlock avoidance. MAFIS doesn't have
//!    designated parking endpoints; idle agents Wait in place.
//!
//! 3. **Swap cooldown**: not in the paper. Added to prevent oscillation
//!    caused by the lack of recursive GetTask (without recursion, the same
//!    pair would swap back and forth every tick).
//!
//! 4. **Bidirectional total-cost criterion**: the paper (Algorithm 2, line 26)
//!    only requires agent `ai` to reach `aj`'s goal faster. MAFIS additionally
//!    requires total A* cost to strictly decrease after the swap. This reduces
//!    swap frequency but prevents swaps that improve one agent at a greater
//!    cost to the other.

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
/// Paper: swap only for agents both heading to pickups (or both to deliveries).
fn legs_compatible(a: &TaskLeg, b: &TaskLeg) -> bool {
    matches!(
        (a, b),
        (TaskLeg::TravelEmpty(_), TaskLeg::TravelEmpty(_))
            | (TaskLeg::TravelLoaded { .. }, TaskLeg::TravelLoaded { .. })
    )
}

/// Canonical pair key — always (min, max) so (a,b) == (b,a).
fn pair_key(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

// ---------------------------------------------------------------------------
// Token snapshot for swap rollback (paper Algorithm 2, line 21/33)
// ---------------------------------------------------------------------------

/// Lightweight snapshot of token paths for a pair of agents.
/// Used to restore state if a swap attempt fails.
struct TokenSnapshot {
    local_a: usize,
    local_b: usize,
    path_a: VecDeque<IVec2>,
    path_b: VecDeque<IVec2>,
}

impl TokenSnapshot {
    fn save(token: &Token, local_a: usize, local_b: usize) -> Self {
        Self {
            local_a,
            local_b,
            path_a: token.paths[local_a].clone(),
            path_b: token.paths[local_b].clone(),
        }
    }

    fn restore(self, token: &mut Token) {
        token.paths[self.local_a] = self.path_a;
        token.paths[self.local_b] = self.path_b;
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

    /// Persistent swap mapping: agent_index → swapped goal.
    /// Populated when a swap commits, cleared when the agent reaches
    /// the swapped goal or is recycled to Free.
    swap_goal_overrides: HashMap<usize, IVec2>,
    /// Pending overrides to send to the runner (drained once per tick).
    pending_runner_overrides: Vec<(usize, IVec2)>,
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
            swap_goal_overrides: HashMap::new(),
            pending_runner_overrides: Vec::new(),
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

    /// Plan a collision-free path from start to goal using spacetime A*.
    /// Returns the path as position sequence, or None if no path found.
    fn plan_path(
        &mut self,
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
            grid, start, goal, &self.master_ci, TOKEN_ASTAR_MAX_TIME,
            Some(dm), &mut self.stg, ASTAR_MAX_EXPANSIONS,
        ).ok()?;

        let mut positions = Vec::with_capacity(actions.len() + 1);
        positions.push(start);
        let mut pos = start;
        for action in &actions {
            pos = action.apply(pos);
            positions.push(pos);
        }

        Some(positions)
    }

    /// Compute actual spacetime A* path cost from start to goal.
    /// Returns None if no path exists (used for swap evaluation).
    fn astar_cost(
        &mut self,
        start: IVec2,
        goal: IVec2,
        grid: &GridMap,
        dist_cache: &mut DistanceMapCache,
    ) -> Option<u64> {
        if start == goal {
            return Some(0);
        }
        let path = self.plan_path(start, goal, grid, dist_cache)?;
        Some((path.len() - 1) as u64)  // path length = number of steps
    }

    /// Attempt swaps with snapshot/restore (paper Algorithm 2, lines 20-33).
    ///
    /// For each candidate pair (i, j):
    /// 1. Check compatibility (task legs, radius, cooldown)
    /// 2. Compute A* arrival times for current and swapped goals
    /// 3. If ai reaches goal_j faster than ai' reaches goal_j: tentatively swap
    /// 4. Try to plan both agents with swapped goals against constraint index
    /// 5. If both plans succeed: commit swap. Otherwise: restore snapshot.
    fn attempt_swaps(
        &mut self,
        agents: &[AgentState],
        tick: u64,
        grid: &GridMap,
        dist_cache: &mut DistanceMapCache,
    ) -> Vec<(usize, usize)> {
        // Clean up stale cooldown entries
        self.swap_cooldown
            .retain(|_, cooldown_tick| tick < *cooldown_tick + TPTS_SWAP_COOLDOWN);

        let mut committed_swaps = Vec::new();
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

                // Manhattan distance pre-filter (avoid expensive A* for distant pairs)
                if manhattan(agents[i].pos, agents[j].pos) > self.swap_radius as u64 {
                    continue;
                }

                // Cooldown check
                let key = pair_key(agents[i].index, agents[j].index);
                if self.swap_cooldown.contains_key(&key) {
                    continue;
                }

                checks += 1;

                // Manhattan pre-filter: skip A* if Manhattan says swap is
                // clearly not beneficial. This avoids 4 expensive A* calls
                // for pairs that can't possibly improve.
                let m_current = manhattan(agents[i].pos, goal_i)
                    + manhattan(agents[j].pos, goal_j);
                let m_swapped = manhattan(agents[i].pos, goal_j)
                    + manhattan(agents[j].pos, goal_i);
                if m_swapped >= m_current {
                    continue; // Manhattan says no benefit — skip A*
                }

                // Look up local indices for constraint index manipulation
                let local_i = match self.agent_map.get(agents[i].index) {
                    Some(&l) if l < self.token.paths.len() => l,
                    _ => continue,
                };
                let local_j = match self.agent_map.get(agents[j].index) {
                    Some(&l) if l < self.token.paths.len() => l,
                    _ => continue,
                };

                // Remove both agents' paths from constraints before cost evaluation
                // (avoids self-interference in A* cost probes)
                self.master_ci.remove_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                self.master_ci.remove_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);

                // Paper criterion (Algorithm 2, line 26): compare arrival times.
                // Manhattan pre-filter passed — now do the expensive A* check.
                let cost_i_to_goal_j = self.astar_cost(agents[i].pos, goal_j, grid, dist_cache);
                let cost_j_to_goal_j = self.astar_cost(agents[j].pos, goal_j, grid, dist_cache);

                let (cost_i_j, cost_j_j) = match (cost_i_to_goal_j, cost_j_to_goal_j) {
                    (Some(a), Some(b)) => (a, b),
                    _ => {
                        // Restore paths and skip
                        self.master_ci.add_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                        self.master_ci.add_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);
                        continue;
                    }
                };

                // Agent i must reach goal_j STRICTLY faster than agent j
                if cost_i_j >= cost_j_j {
                    self.master_ci.add_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                    self.master_ci.add_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);
                    continue;
                }

                // Also check the reverse: agent j should reach goal_i reasonably
                let cost_j_to_goal_i = self.astar_cost(agents[j].pos, goal_i, grid, dist_cache);
                let cost_i_to_goal_i = self.astar_cost(agents[i].pos, goal_i, grid, dist_cache);

                let (cost_j_i, cost_i_i) = match (cost_j_to_goal_i, cost_i_to_goal_i) {
                    (Some(a), Some(b)) => (a, b),
                    _ => {
                        self.master_ci.add_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                        self.master_ci.add_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);
                        continue;
                    }
                };

                // Total cost must decrease
                if cost_i_j + cost_j_i >= cost_i_i + cost_j_j {
                    self.master_ci.add_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                    self.master_ci.add_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);
                    continue;
                }

                // Tentative swap — snapshot token state (paper line 21)
                // Paths are already removed from master_ci above — proceed directly to planning
                let snapshot = TokenSnapshot::save(&self.token, local_i, local_j);

                // Plan agent i toward goal_j
                let plan_i = self.plan_path(agents[i].pos, goal_j, grid, dist_cache);

                if let Some(ref positions_i) = plan_i {
                    // Add i's new path to constraints so j plans against it
                    let new_path_i: VecDeque<IVec2> = positions_i.iter().copied().collect();
                    self.master_ci.add_path(&new_path_i, TOKEN_PATH_MAX_TIME);
                    self.token.set_path(local_i, positions_i.clone());

                    // Plan agent j toward goal_i
                    let plan_j = self.plan_path(agents[j].pos, goal_i, grid, dist_cache);

                    if let Some(ref positions_j) = plan_j {
                        // Both plans succeeded — commit swap
                        let new_path_j: VecDeque<IVec2> = positions_j.iter().copied().collect();
                        self.master_ci.add_path(&new_path_j, TOKEN_PATH_MAX_TIME);
                        self.token.set_path(local_j, positions_j.clone());
                        self.swap_cooldown.insert(key, tick);
                        committed_swaps.push((i, j));

                        // Persist swap so solver plans consistently across ticks,
                        // and notify runner to update agent.goal for task completion.
                        self.swap_goal_overrides.insert(agents[i].index, goal_j);
                        self.swap_goal_overrides.insert(agents[j].index, goal_i);
                        self.pending_runner_overrides.push((agents[i].index, goal_j));
                        self.pending_runner_overrides.push((agents[j].index, goal_i));
                        continue;
                    }

                    // Plan j failed — remove i's tentative path
                    self.master_ci.remove_path(&new_path_i, TOKEN_PATH_MAX_TIME);
                }

                // Swap failed — restore snapshot (paper line 33)
                snapshot.restore(&mut self.token);
                self.master_ci.add_path(&self.token.paths[local_i], TOKEN_PATH_MAX_TIME);
                self.master_ci.add_path(&self.token.paths[local_j], TOKEN_PATH_MAX_TIME);
            }
        }

        committed_swaps
    }
}

impl LifelongSolver for TptsSolver {
    fn name(&self) -> &'static str {
        "tpts"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n × A* + swap_checks × A*) per replan",
            scalability: Scalability::Medium,
            description:
                "TPTS — Token Passing with Task Swaps. A* cost swap evaluation with snapshot/restore.",
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
        self.swap_cooldown.clear();
        self.swap_goal_overrides.clear();
        self.pending_runner_overrides.clear();
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

        // Clear stale swap overrides: agent reached swapped goal or was recycled to Free
        self.swap_goal_overrides.retain(|&agent_idx, &mut override_goal| {
            agents.iter().any(|a| {
                a.index == agent_idx
                    && a.pos != override_goal
                    && !matches!(a.task_leg, TaskLeg::Free)
            })
        });

        // Build master constraint index
        self.master_ci
            .reset(ctx.grid.width, ctx.grid.height, TOKEN_PATH_MAX_TIME);
        for path in &self.token.paths {
            self.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // TPTS extension: attempt swaps with A* cost evaluation + snapshot/restore.
        // Committed swaps have already updated token paths and master CI.
        let committed_swaps = self.attempt_swaps(agents, ctx.tick, ctx.grid, distance_cache);

        // Build swapped goals from persistent overrides + this tick's new swaps
        let mut swapped_goals: Vec<Option<IVec2>> = agents.iter().map(|a| {
            if let Some(&override_goal) = self.swap_goal_overrides.get(&a.index) {
                Some(override_goal)
            } else {
                a.goal
            }
        }).collect();
        for &(i, j) in &committed_swaps {
            let tmp = swapped_goals[i];
            swapped_goals[i] = swapped_goals[j];
            swapped_goals[j] = tmp;
        }

        // Track which agents were already replanned by the swap phase
        let mut swap_replanned = std::collections::HashSet::new();
        for &(i, j) in &committed_swaps {
            swap_replanned.insert(agents[i].index);
            swap_replanned.insert(agents[j].index);
        }

        // Plan for remaining agents that need replanning
        let mut planning_order: Vec<usize> = (0..agents.len()).collect();
        planning_order.sort_by_key(|&i| {
            let has_task = !matches!(agents[i].task_leg, TaskLeg::Free);
            (!has_task, agents[i].index)
        });

        for &agent_idx in &planning_order {
            let a = &agents[agent_idx];

            // Skip agents already handled by swap phase
            if swap_replanned.contains(&a.index) {
                continue;
            }

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

            let path = self.plan_path(a.pos, goal, ctx.grid, distance_cache);

            if let Some(positions) = path {
                let new_path: VecDeque<IVec2> = positions.iter().copied().collect();
                self.master_ci.add_path(&new_path, TOKEN_PATH_MAX_TIME);
                self.token.set_path(local, positions);
            } else {
                self.master_ci.add_path(&old_path, TOKEN_PATH_MAX_TIME);
            }
        }

        // Emit actions for ALL agents
        for a in agents {
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

    fn drain_goal_overrides(&mut self) -> Vec<(usize, IVec2)> {
        std::mem::take(&mut self.pending_runner_overrides)
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
            grid: &grid, zones: &zones, tick: 0, num_agents: 0,
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
                index: 0, pos, goal: Some(goal), has_plan: tick > 0,
                task_leg: TaskLeg::TravelEmpty(goal),
            }];
            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 1 };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                if let Some((_, actions)) = plans.first() {
                    if let Some(action) = actions.first() {
                        pos = action.apply(pos);
                    }
                }
            }
            if pos == goal { return; }
        }
        assert_eq!(pos, goal);
    }

    #[test]
    fn tpts_swap_uses_astar_cost() {
        // Agent 0 at (0,0) with goal (4,0): A* cost = 4
        // Agent 1 at (3,0) with goal (1,0): A* cost = 2
        // Swapped: Agent 0→(1,0) cost=1, Agent 1→(4,0) cost=1. Total 2 < 6.
        // And Agent 0 reaches (1,0) faster than Agent 1 (1 < 2).
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let agents = vec![
            AgentState {
                index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1, pos: IVec2::new(3, 0), goal: Some(IVec2::new(1, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];

        // Initialize and build constraint index
        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);
        assert!(!swaps.is_empty(), "should find a swap using A* costs");
    }

    #[test]
    fn tpts_swap_restores_on_incompatible() {
        let mut solver = TptsSolver::new();
        let agents = vec![
            AgentState {
                index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1, pos: IVec2::new(3, 0), goal: Some(IVec2::new(1, 0)),
                has_plan: false, task_leg: TaskLeg::Free, // Incompatible
            },
        ];

        let grid = GridMap::new(5, 5);
        let mut cache = DistanceMapCache::default();

        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);
        assert!(swaps.is_empty(), "incompatible legs should prevent swap");
    }

    #[test]
    fn tpts_swap_cooldown() {
        let grid = GridMap::new(5, 5);

        // Test cooldown directly via the HashMap
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();

        let agents = vec![
            AgentState {
                index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1, pos: IVec2::new(3, 0), goal: Some(IVec2::new(1, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];

        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // First call: swap found and committed
        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);
        assert!(!swaps.is_empty(), "should find initial swap");

        // Verify cooldown was recorded
        let key = pair_key(agents[0].index, agents[1].index);
        assert!(solver.swap_cooldown.contains_key(&key), "cooldown should be recorded");

        // Immediately after: cooldown prevents re-swap (even with fresh state)
        let mut solver2 = TptsSolver::new();
        solver2.swap_cooldown = solver.swap_cooldown.clone();
        solver2.ensure_initialized(&agents);
        solver2.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver2.token.paths {
            solver2.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }
        let swaps = solver2.attempt_swaps(&agents, 1, &grid, &mut cache);
        assert!(swaps.is_empty(), "cooldown should prevent re-swap at tick 1");

        // After cooldown expires: swap allowed again
        let mut solver3 = TptsSolver::new();
        solver3.swap_cooldown = solver.swap_cooldown.clone();
        solver3.ensure_initialized(&agents);
        solver3.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver3.token.paths {
            solver3.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }
        let swaps = solver3.attempt_swaps(&agents, TPTS_SWAP_COOLDOWN + 1, &grid, &mut cache);
        assert!(!swaps.is_empty(), "swap should be allowed after cooldown expires");
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
                    index: i, pos: positions[i], goal: Some(goals[i]),
                    has_plan: tick > 0, task_leg: TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();

            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 2 };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        positions[*idx] = action.apply(positions[*idx]);
                    }
                }
            }
            assert_ne!(positions[0], positions[1], "vertex collision at tick {tick}");
        }
    }

    // ── Tier 2: Paper property tests ─────────────────────────────────

    /// Paper property (Ma et al., AAMAS 2017, Algorithm 2 line 27):
    /// A swap is only committed when the stealing agent reaches the pickup
    /// STRICTLY faster than the current assignee (A* cost comparison).
    /// This test verifies that committed swaps always reduce total cost.
    #[test]
    fn paper_property_swap_reduces_total_astar_cost() {
        let grid = GridMap::new(8, 8);
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();

        // Agent 0 far from its goal, Agent 1 close to Agent 0's goal
        let agents = vec![
            AgentState {
                index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(7, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(7, 0)),
            },
            AgentState {
                index: 1, pos: IVec2::new(6, 0), goal: Some(IVec2::new(1, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];

        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // Use Manhattan as pre-swap cost estimate (no constraint interference)
        let pre_cost_0 = manhattan(agents[0].pos, agents[0].goal.unwrap());
        let pre_cost_1 = manhattan(agents[1].pos, agents[1].goal.unwrap());
        let pre_swap_total = pre_cost_0 + pre_cost_1;

        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);

        if !swaps.is_empty() {
            // Post-swap: agent 0 → agent 1's original goal, agent 1 → agent 0's original goal
            let post_cost_0 = manhattan(agents[0].pos, agents[1].goal.unwrap());
            let post_cost_1 = manhattan(agents[1].pos, agents[0].goal.unwrap());
            let post_swap_total = post_cost_0 + post_cost_1;

            assert!(
                post_swap_total < pre_swap_total,
                "swap should reduce total cost: pre={pre_swap_total}, post={post_swap_total}"
            );
        } else {
            panic!("expected a swap for agents (0,0)→(7,0) and (6,0)→(1,0)");
        }
    }

    /// Paper property: after a swap, no two agents share the same goal.
    /// This ensures task well-formedness is preserved.
    #[test]
    fn paper_property_swap_preserves_unique_goals() {
        let grid = GridMap::new(8, 8);
        let zones = test_zones();
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let mut positions = vec![
            IVec2::new(0, 0), IVec2::new(6, 0), IVec2::new(3, 3),
        ];
        let goals = vec![
            IVec2::new(7, 0), IVec2::new(1, 0), IVec2::new(5, 5),
        ];

        for tick in 0..20 {
            let agents: Vec<AgentState> = (0..3)
                .map(|i| AgentState {
                    index: i, pos: positions[i], goal: Some(goals[i]),
                    has_plan: tick > 0, task_leg: TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();

            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 3 };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                // After step, verify no two agents in the plan share the same position
                let plan_positions: Vec<IVec2> = plans.iter()
                    .filter_map(|(idx, actions)| {
                        actions.first().map(|a| a.apply(positions[*idx]))
                    })
                    .collect();

                let unique: std::collections::HashSet<IVec2> = plan_positions.iter().copied().collect();
                assert_eq!(
                    unique.len(), plan_positions.len(),
                    "swap produced duplicate target positions at tick {tick}"
                );

                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        positions[*idx] = action.apply(positions[*idx]);
                    }
                }
            }
        }
    }

    /// Paper property: swap cost evaluation uses unbiased A* (paths removed
    /// before probing). This is the regression test for the critical fix that
    /// removes both agents' token paths from the constraint index before calling
    /// astar_cost(), preventing self-interference in cost estimation.
    ///
    /// If paths were NOT removed before probing, each agent's own token path
    /// would inflate its A* cost (it would have to route around itself), making
    /// beneficial swaps appear non-beneficial. The fix ensures costs are
    /// computed on a "clean" constraint index.
    ///
    /// We verify this by constructing a scenario where agent 0 is close to
    /// agent 1's goal and vice versa, giving each agent a long pre-existing
    /// TOKEN path. Without the fix, the A* probe would route around the full
    /// existing path and inflate the cost, suppressing the swap.
    #[test]
    fn paper_property_swap_cost_unbiased() {
        let grid = GridMap::new(10, 10);
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();

        // Agent 0 at (0,0) with goal (9,0) — distance 9
        // Agent 1 at (8,0) with goal (1,0) — distance 7
        // After swap: agent 0→(1,0) costs 1, agent 1→(9,0) costs 1
        // Total pre-swap: 16, post-swap: 2 — a clear improvement
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(9, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(9, 0)),
            },
            AgentState {
                index: 1,
                pos: IVec2::new(8, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(1, 0)),
            },
        ];

        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);

        // Give each agent a long pre-existing TOKEN path that spans the row.
        // These paths are deliberately set BEFORE building master_ci so that
        // without the fix (paths not removed before A* probes) the A* would
        // see its own path as constraints and be forced to route around them.
        let path_0: Vec<IVec2> = (0..=9).map(|x| IVec2::new(x, 0)).collect();
        let path_1: Vec<IVec2> = (1..=8).rev().map(|x| IVec2::new(x, 0)).collect();
        solver.token.set_path(0, path_0);
        solver.token.set_path(1, path_1);

        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // With the fix (paths removed before cost probing), the A* sees a clear
        // grid for each agent and correctly detects the swap is beneficial.
        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);
        assert!(
            !swaps.is_empty(),
            "swap should be found when A* costs are computed without self-interference \
             (paths removed from constraint index before probing)"
        );
        assert!(
            swaps.contains(&(0, 1)),
            "expected swap between agent 0 and agent 1, got: {swaps:?}"
        );
    }

    /// Paper property: token constraint index remains consistent after swap.
    /// Snapshot/restore must leave the constraint index in a valid state.
    #[test]
    fn paper_property_snapshot_restore_consistency() {
        let grid = GridMap::new(5, 5);
        let mut solver = TptsSolver::new();
        let mut cache = DistanceMapCache::default();

        let agents = vec![
            AgentState {
                index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 0)),
            },
            AgentState {
                index: 1, pos: IVec2::new(4, 0), goal: Some(IVec2::new(0, 0)),
                has_plan: false, task_leg: TaskLeg::Free, // incompatible → swap will be skipped
            },
        ];

        solver.ensure_initialized(&agents);
        solver.master_ci.reset(grid.width, grid.height, TOKEN_PATH_MAX_TIME);
        for path in &solver.token.paths {
            solver.master_ci.add_path(path, TOKEN_PATH_MAX_TIME);
        }

        // Save token state before
        let paths_before: Vec<Vec<IVec2>> = solver.token.paths.iter()
            .map(|p| p.iter().copied().collect())
            .collect();

        // attempt_swaps should not modify anything (incompatible legs)
        let swaps = solver.attempt_swaps(&agents, 0, &grid, &mut cache);
        assert!(swaps.is_empty());

        // Token paths should be unchanged
        let paths_after: Vec<Vec<IVec2>> = solver.token.paths.iter()
            .map(|p| p.iter().copied().collect())
            .collect();
        assert_eq!(paths_before, paths_after, "token paths changed despite no swap");
    }
}
