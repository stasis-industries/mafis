//! LaCAM3LifelongSolver — wraps the LaCAM3 one-shot planner in a lifelong loop.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/planner.cpp (one-shot)
//! Lifelong wrapper is MAFIS-specific — lacam3 has no equivalent.
//!
//! ## Wrapper logic
//!
//! LaCAM3 is a single-shot solver: given (starts, goals), it produces a
//! Solution = sequence of Configs. To use it as a lifelong solver in MAFIS,
//! we wrap it as follows:
//!
//! 1. Maintain a cached `Plan: Vec<Config>` indexed by execution timestep
//!    (0 = current state, 1 = next, ...).
//! 2. Each tick, the wrapper:
//!    - Extracts the next per-agent action from `plan[0] → plan[1]`
//!    - Returns those as `StepResult::Replan`
//!    - Pops the front of the plan (advancing the cursor)
//! 3. Replan triggers (recompute the cached plan via `Planner::solve`):
//!    - First call (no plan exists)
//!    - Any agent's goal changed since last replan
//!    - Plan exhausted (cursor reached end)
//!    - Failsafe: every `LACAM3_REPLAN_INTERVAL` ticks
//!    - Position drift: any agent's actual pos diverged from expected (e.g. fault)

use bevy::prelude::*;
use std::collections::HashMap;

use crate::core::seed::SeededRng;
use crate::solver::lifelong::{
    AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult,
};
use crate::solver::shared::heuristics::DistanceMapCache;
use crate::solver::shared::traits::{Optimality, Scalability, SolverInfo};

use super::dist_table::DistTable;
use super::instance::{Instance, Solution, id_to_pos};
use super::planner::{Planner, PlannerConfig};

/// Replan failsafe: recompute plan every K ticks even if nothing else changed.
pub const LACAM3_REPLAN_INTERVAL: u64 = 30;

/// Maximum search iterations per replan call (analog of lacam3's deadline).
pub const LACAM3_MAX_ITERS: usize = 5_000;

pub struct LaCAM3LifelongSolver {
    /// Cached plan (sequence of Configs). `plan[0]` is current state.
    /// Plan is indexed by SLOT (position in the agents slice), not external
    /// `AgentState.index`, because the underlying `Instance` is built from
    /// `agents.iter()` order each replan.
    plan: Solution,
    /// Cached agent goals from last replan, keyed by external `AgentState.index`.
    /// Used to detect goal changes that trigger replan.
    last_goals: HashMap<usize, IVec2>,
    /// Number of agents at last replan; if it changes mid-run, replan.
    last_agent_count: usize,
    /// Tick of last replan, for the K-tick failsafe.
    last_replan_tick: u64,
    /// Per-tick scratch buffer for action plans.
    plan_buffer: Vec<AgentPlan>,
    /// Grid width remembered from last replan, for cell-id ↔ IVec2 conversion.
    last_grid_width: i32,
    /// Whether the solver has produced any plan yet.
    initialized: bool,
}

impl Default for LaCAM3LifelongSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LaCAM3LifelongSolver {
    pub fn new() -> Self {
        Self {
            plan: Vec::new(),
            last_goals: HashMap::new(),
            last_agent_count: 0,
            last_replan_tick: 0,
            plan_buffer: Vec::new(),
            last_grid_width: 0,
            initialized: false,
        }
    }

    /// Decide whether to trigger a replan this tick.
    /// Uses slot index `i` (position in `agents` slice), not `a.index`,
    /// because the plan vector is built from `agents.iter()` order.
    fn needs_replan(&self, agents: &[AgentState], tick: u64) -> bool {
        if !self.initialized {
            return true;
        }
        if self.plan.len() <= 1 {
            return true; // exhausted
        }
        if tick.saturating_sub(self.last_replan_tick) >= LACAM3_REPLAN_INTERVAL {
            return true;
        }
        if agents.len() != self.last_agent_count {
            return true; // agent set changed
        }
        // Goal change detection — keyed by external agent id.
        for a in agents {
            if let Some(goal) = a.goal {
                match self.last_goals.get(&a.index) {
                    Some(&old) if old == goal => {}
                    _ => return true,
                }
            }
        }
        // Position sync: actual pos vs expected pos at slot i.
        if let Some(expected) = self.plan.first() {
            for (i, a) in agents.iter().enumerate() {
                if i < expected.len() {
                    let expected_pos = id_to_pos(expected[i], self.last_grid_width);
                    if expected_pos != a.pos {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn replan(&mut self, ctx: &SolverContext, agents: &[AgentState]) {
        let n = agents.len();
        if n == 0 {
            self.plan.clear();
            return;
        }

        // Build start and goal vectors. For idle agents (no goal), set goal=start
        // so they stay in place.
        //
        // **Goal deduplication**: lacam3 is a one-shot MAPF planner that requires
        // distinct goal cells (no two agents at the same cell in the goal config).
        // In lifelong/MAPD workloads, multiple agents commonly share an endpoint
        // (e.g., the same delivery cell). To make the lacam3 instance solvable,
        // we keep the closest agent's true goal and assign the others their
        // current position as a "wait until next replan" goal. The next replan
        // (triggered when one agent's goal changes) will let the deferred agents
        // pursue their real goals.
        let mut starts: Vec<IVec2> = Vec::with_capacity(n);
        let mut goals: Vec<IVec2> = Vec::with_capacity(n);
        for a in agents {
            starts.push(a.pos);
            goals.push(a.goal.unwrap_or(a.pos));
        }

        // Deduplicate goals: when N agents share a goal, the closest keeps it,
        // the others get their current position as a temporary goal.
        let mut goal_owner: HashMap<IVec2, (usize, i32)> = HashMap::new();
        for i in 0..n {
            // Skip agents whose goal is already their position (idle).
            if goals[i] == starts[i] {
                continue;
            }
            let dist = (goals[i].x - starts[i].x).abs() + (goals[i].y - starts[i].y).abs();
            match goal_owner.get(&goals[i]) {
                Some(&(owner, owner_dist)) if owner_dist <= dist => {
                    // Existing owner is closer; defer this agent.
                    let _ = owner;
                    goals[i] = starts[i];
                }
                _ => {
                    // This agent becomes the new owner.
                    if let Some((prev_owner, _)) = goal_owner.insert(goals[i], (i, dist)) {
                        // Defer the previous owner.
                        goals[prev_owner] = starts[prev_owner];
                    }
                }
            }
        }

        // Update caches.
        self.last_goals.clear();
        for a in agents {
            if let Some(g) = a.goal {
                self.last_goals.insert(a.index, g);
            }
        }
        self.last_agent_count = n;
        self.last_grid_width = ctx.grid.width;
        self.last_replan_tick = ctx.tick;

        // Build instance and run lacam3.
        let ins = Instance::new(ctx.grid, starts, goals);
        let dt = DistTable::new(ctx.grid, &ins);
        let cfg = PlannerConfig {
            max_iters: LACAM3_MAX_ITERS,
            // Disable scatter for the lifelong inner-loop case to keep
            // per-replan cost predictable. Scatter pays off on hard one-shot
            // instances; lifelong replans every K ticks so single-shot speed
            // matters more than absolute optimality.
            flg_scatter: false,
            ..PlannerConfig::default()
        };
        let mut planner = Planner::new(&ins, &dt, cfg, ctx.tick);
        let solution = planner.solve();

        if solution.is_empty() {
            // Solver failed — keep old plan if any.
            return;
        }

        self.plan = solution;
        self.initialized = true;
    }
}

impl LifelongSolver for LaCAM3LifelongSolver {
    fn name(&self) -> &'static str {
        "lacam3_lifelong"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(LaCAM* search per replan, amortized over K ticks)",
            scalability: Scalability::High,
            description: "LaCAM3 — Engineered LaCAM* (AAMAS 2024) wrapped in a lifelong replan loop. Configuration-space search with PIBT generator.",
            source: "Okumura, AAMAS 2024",
            recommended_max_agents: Some(1000),
        }
    }

    fn reset(&mut self) {
        self.plan.clear();
        self.last_goals.clear();
        self.last_agent_count = 0;
        self.last_replan_tick = 0;
        self.plan_buffer.clear();
        self.last_grid_width = 0;
        self.initialized = false;
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        _distance_cache: &mut DistanceMapCache,
        _rng: &mut SeededRng,
    ) -> StepResult<'a> {
        if agents.is_empty() {
            self.plan_buffer.clear();
            return StepResult::Replan(&self.plan_buffer);
        }

        if self.needs_replan(agents, ctx.tick) {
            self.replan(ctx, agents);
        }

        // Advance the cached plan.
        if self.plan.len() < 2 {
            // No usable next step — emit waits for everyone.
            self.plan_buffer.clear();
            for a in agents {
                self.plan_buffer.push((
                    a.index,
                    smallvec::smallvec![crate::core::action::Action::Wait],
                ));
            }
            return StepResult::Replan(&self.plan_buffer);
        }

        let cur = self.plan[0].clone();
        let next = self.plan[1].clone();
        let grid_width = ctx.grid.width;

        self.plan_buffer.clear();
        for (i, a) in agents.iter().enumerate() {
            // Slot index `i` matches the lacam3 plan vector layout (built from
            // `agents.iter()` order). External `a.index` is what the runner
            // expects in the AgentPlan tuple.
            if i >= cur.len() || i >= next.len() {
                self.plan_buffer.push((
                    a.index,
                    smallvec::smallvec![crate::core::action::Action::Wait],
                ));
                continue;
            }
            let from = id_to_pos(cur[i], grid_width);
            let to = id_to_pos(next[i], grid_width);
            let action = crate::solver::shared::heuristics::delta_to_action(from, to);
            self.plan_buffer.push((a.index, smallvec::smallvec![action]));
        }

        // Pop the front so plan[0] becomes the expected next-tick state.
        self.plan.remove(0);

        StepResult::Replan(&self.plan_buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::task::TaskLeg;
    use crate::core::topology::ZoneMap;
    use std::collections::HashMap as StdHashMap;

    fn test_zones() -> ZoneMap {
        ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(4, 4)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: StdHashMap::new(),
            queue_lines: Vec::new(),
        }
    }

    #[test]
    fn lacam3_lifelong_empty_agents() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = LaCAM3LifelongSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 0 };
        let result = solver.step(&ctx, &[], &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(plans) if plans.is_empty()));
    }

    #[test]
    fn lacam3_lifelong_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = LaCAM3LifelongSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let mut pos = IVec2::ZERO;
        let goal = IVec2::new(4, 4);

        for tick in 0..30 {
            let agents = vec![AgentState {
                index: 0,
                pos,
                goal: Some(goal),
                has_plan: tick > 0,
                task_leg: TaskLeg::TravelEmpty(goal),
            }];
            let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: 1 };
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                if let Some((_, actions)) = plans.first() {
                    if let Some(action) = actions.first() {
                        let new_pos = action.apply(pos);
                        assert!(grid.is_walkable(new_pos), "moved to obstacle at tick {tick}");
                        pos = new_pos;
                    }
                }
            }
            if pos == goal {
                return;
            }
        }
        assert_eq!(pos, goal, "agent should reach goal within 30 ticks");
    }

    #[test]
    fn lacam3_lifelong_two_agents_no_collision() {
        let grid = GridMap::new(7, 7);
        let zones = test_zones();
        let mut solver = LaCAM3LifelongSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let mut positions = vec![IVec2::new(0, 3), IVec2::new(6, 3)];
        let goals = vec![IVec2::new(6, 3), IVec2::new(0, 3)];

        for tick in 0..40 {
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
            if let StepResult::Replan(plans) = solver.step(&ctx, &agents, &mut cache, &mut rng) {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        let new_pos = action.apply(positions[*idx]);
                        assert!(grid.is_walkable(new_pos));
                        positions[*idx] = new_pos;
                    }
                }
            }
            if positions[0] == positions[1] {
                panic!("vertex collision at tick {tick}: {:?}", positions);
            }
        }
    }

    /// End-to-end validation: lacam3 runs through the full MAFIS experiment
    /// runner on warehouse_large with 20 agents, 200 ticks, no faults.
    /// Sub-step 4h of solver-refocus.
    #[test]
    fn lacam3_lifelong_warehouse_large_baseline() {
        use crate::experiment::config::ExperimentConfig;
        use crate::experiment::runner::run_single_experiment;

        let config = ExperimentConfig {
            solver_name: "lacam3_lifelong".into(),
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
        let tasks = result.baseline_metrics.total_tasks;
        eprintln!(
            "lacam3_lifelong_warehouse_large_baseline: tp={tp:.4} tasks/tick, total_tasks={tasks}"
        );
        // lacam3 should produce non-zero throughput on this instance.
        assert!(
            tp > 0.0,
            "lacam3_lifelong produced zero throughput on warehouse_large/20 agents/200 ticks"
        );
    }
}
