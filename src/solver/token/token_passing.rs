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

        // Advance token: pop the front of every path, simulating that all agents
        // moved to their TOKEN[i][1] position. This is the relative-time
        // equivalent of pibt2's `current_timestep++` (tp.cpp line 165 `P->update()`).
        self.token.advance();

        // Sync actual positions — MAFIS-specific fault handling.
        // pibt2 has no equivalent because it's a finite-task experiment with
        // no fault model. If an agent didn't move as expected (fault, latency,
        // collision recovery), its token diverges from reality; reset to
        // [actual_pos] so the next replan starts fresh.
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
        assert!(registry.find("warehouse_large").is_some(), "warehouse_large.json missing");

        let config = ExperimentConfig {
            solver_name: "token_passing".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 20,
            seed: 42,
            tick_count: 200,
            custom_map: None,
        };
        let result = run_single_experiment(&config);
        let tp = result.baseline_metrics.avg_throughput;
        eprintln!("token_passing_throughput_regression: tp={tp:.4} tasks/tick");
        // Token Passing on warehouse_large at 20 agents should produce
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
