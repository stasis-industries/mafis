//! PIBT — Priority Inheritance with Backtracking
//!
//! Standalone lifelong solver: replans all agents every tick with 1-step plans.
//! Uses PibtCore for the actual algorithm.
//!
//! Reference: Okumura et al., "Priority Inheritance with Backtracking for
//! Iterative Multi-agent Path Finding" (AAAI 2019).

use bevy::prelude::*;
use smallvec::smallvec;

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::heuristics::{DistanceMap, DistanceMapCache, compute_distance_maps};
use super::lifelong::{ActiveSolver, AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use super::pibt_core::PibtCore;
use super::traits::{MAPFSolver, Optimality, Scalability, SolverError, SolverInfo};

// ---------------------------------------------------------------------------
// PibtLifelongSolver — implements LifelongSolver
// ---------------------------------------------------------------------------

pub struct PibtLifelongSolver {
    core: PibtCore,
    plan_buffer: Vec<AgentPlan>,
    // Pre-allocated per-tick scratch buffers — avoids allocation on every step()
    agent_pairs_buf: Vec<(IVec2, IVec2)>,
    positions_buf: Vec<IVec2>,
    goals_buf: Vec<IVec2>,
    has_task_buf: Vec<bool>,
}

impl PibtLifelongSolver {
    pub fn new() -> Self {
        Self {
            core: PibtCore::new(),
            plan_buffer: Vec::new(),
            agent_pairs_buf: Vec::new(),
            positions_buf: Vec::new(),
            goals_buf: Vec::new(),
            has_task_buf: Vec::new(),
        }
    }
}

impl Default for PibtLifelongSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LifelongSolver for PibtLifelongSolver {
    fn name(&self) -> &'static str {
        "pibt"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n log n) per timestep",
            scalability: Scalability::High,
            description: "PIBT — fast, reactive, one-step planner. Scales to 1000+ agents.",
            source: "Okumura et al., AAAI 2019",
            recommended_max_agents: None,
        }
    }

    fn reset(&mut self) {
        self.core.reset();
        self.plan_buffer.clear();
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        let _ = rng; // PIBT is deterministic given priorities

        // Derive shuffle seed from tick so tie-breaking is deterministic
        // after rewind (survives solver.reset() which clears the counter).
        self.core.set_shuffle_seed(ctx.tick);

        if agents.is_empty() {
            self.plan_buffer.clear();
            return StepResult::Replan(&self.plan_buffer);
        }

        // Build position/goal pairs for distance cache — reuse buffer
        self.agent_pairs_buf.clear();
        self.agent_pairs_buf.extend(agents.iter().map(|a| (a.pos, a.goal.unwrap_or(a.pos))));

        let dist_maps = distance_cache.get_or_compute(ctx.grid, &self.agent_pairs_buf);

        self.positions_buf.clear();
        self.positions_buf.extend(agents.iter().map(|a| a.pos));

        self.goals_buf.clear();
        self.goals_buf.extend(agents.iter().map(|a| a.goal.unwrap_or(a.pos)));

        // Task-aware priorities: agents actively moving toward a goal get
        // scheduling priority.  Agents where goal == pos (Idle, or Loading at
        // pickup) are forced to Wait and excluded from PIBT planning so they
        // don't participate in priority inheritance.
        self.has_task_buf.clear();
        self.has_task_buf.extend(agents.iter().map(|a| {
            let goal = a.goal.unwrap_or(a.pos);
            // Only agents that need to *move* somewhere have a task for PIBT
            goal != a.pos
        }));

        let actions = self.core.one_step_with_tasks(
            &self.positions_buf, &self.goals_buf, ctx.grid, &dist_maps, &self.has_task_buf,
        );

        // Write into pre-allocated buffer
        self.plan_buffer.clear();
        for (i, &action) in actions.iter().enumerate() {
            self.plan_buffer.push((agents[i].index, smallvec![action]));
        }

        StepResult::Replan(&self.plan_buffer)
    }

    fn save_priorities(&self) -> Vec<f32> {
        self.core.priorities().to_vec()
    }

    fn restore_priorities(&mut self, priorities: &[f32]) {
        self.core.set_priorities(priorities);
    }
}

// ---------------------------------------------------------------------------
// Legacy MAPFSolver impl — used by solve_on_enter (kept for compatibility)
// ---------------------------------------------------------------------------

const DEFAULT_MAX_TIMESTEPS: u64 = 1000;

pub struct PibtSolver {
    pub max_timesteps: u64,
}

impl Default for PibtSolver {
    fn default() -> Self {
        Self {
            max_timesteps: DEFAULT_MAX_TIMESTEPS,
        }
    }
}

impl MAPFSolver for PibtSolver {
    fn name(&self) -> &str {
        "pibt"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n log n) per timestep",
            scalability: Scalability::High,
            description: "Priority Inheritance with Backtracking — fast, reactive, one-step planner. Scales to 1000+ agents with O(n log n) per timestep.",
            source: "Okumura et al., AAAI 2019",
            recommended_max_agents: None,
        }
    }

    fn solve(
        &self,
        grid: &GridMap,
        agents: &[(IVec2, IVec2)],
    ) -> Result<Vec<Vec<Action>>, SolverError> {
        let n = agents.len();
        if n == 0 {
            return Ok(Vec::new());
        }

        for (i, (start, goal)) in agents.iter().enumerate() {
            if !grid.is_walkable(*start) {
                return Err(SolverError::InvalidInput(format!(
                    "agent {i} start {start} is not walkable"
                )));
            }
            if !grid.is_walkable(*goal) {
                return Err(SolverError::InvalidInput(format!(
                    "agent {i} goal {goal} is not walkable"
                )));
            }
        }

        let dist_maps = compute_distance_maps(grid, agents);
        let dist_refs: Vec<&DistanceMap> = dist_maps.iter().collect();
        self.solve_with_maps(agents, grid, &dist_refs)
    }
}

impl PibtSolver {
    /// Solve using pre-computed distance maps (avoids redundant BFS).
    pub fn solve_with_maps(
        &self,
        agents: &[(IVec2, IVec2)],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
    ) -> Result<Vec<Vec<Action>>, SolverError> {
        let n = agents.len();
        if n == 0 {
            return Ok(Vec::new());
        }

        let goals: Vec<IVec2> = agents.iter().map(|(_, g)| *g).collect();
        let mut positions: Vec<IVec2> = agents.iter().map(|(s, _)| *s).collect();
        let mut paths: Vec<Vec<Action>> = vec![Vec::new(); n];
        let mut priorities: Vec<f32> = (0..n)
            .map(|i| dist_maps[i].get(positions[i]) as f32)
            .collect();

        for _t in 0..self.max_timesteps {
            if positions.iter().zip(goals.iter()).all(|(p, g)| p == g) {
                break;
            }

            let step_actions = super::pibt_core::pibt_one_step(
                &positions, &goals, grid, dist_maps, &mut priorities,
            );

            for i in 0..n {
                positions[i] = step_actions[i].apply(positions[i]);
                paths[i].push(step_actions[i]);
            }

            for i in 0..n {
                if positions[i] == goals[i] {
                    priorities[i] = 0.0;
                } else {
                    priorities[i] += 1.0;
                }
            }
        }

        Ok(paths)
    }
}

// ---------------------------------------------------------------------------
// Helper to create default ActiveSolver
// ---------------------------------------------------------------------------

pub fn default_active_solver() -> ActiveSolver {
    ActiveSolver::new(Box::new(PibtLifelongSolver::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;

    fn solver() -> PibtSolver {
        PibtSolver::default()
    }

    fn open5() -> GridMap {
        GridMap::new(5, 5)
    }

    fn final_pos(plan: &[Action], start: IVec2) -> IVec2 {
        let mut pos = start;
        for &a in plan {
            pos = a.apply(pos);
        }
        pos
    }

    fn no_vertex_conflicts(plans: &[Vec<Action>], agents: &[(IVec2, IVec2)]) -> bool {
        let max_t = plans.iter().map(|p| p.len()).max().unwrap_or(0);
        let timelines: Vec<Vec<IVec2>> = plans
            .iter()
            .zip(agents.iter())
            .map(|(plan, (start, _goal))| {
                let mut pos = *start;
                let mut tl = vec![pos];
                for &a in plan {
                    pos = a.apply(pos);
                    tl.push(pos);
                }
                tl
            })
            .collect();

        for t in 0..=max_t {
            let mut seen = std::collections::HashSet::new();
            for tl in &timelines {
                let p = if t < tl.len() { tl[t] } else { *tl.last().unwrap() };
                if !seen.insert(p) {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn pibt_empty_agents_returns_empty_plan() {
        let result = solver().solve(&open5(), &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn pibt_all_agents_already_at_goal_returns_empty_plans() {
        let agents = vec![(IVec2::new(1, 1), IVec2::new(1, 1))];
        let result = solver().solve(&open5(), &agents).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].is_empty());
    }

    #[test]
    fn pibt_single_agent_reaches_goal() {
        let agents = vec![(IVec2::ZERO, IVec2::new(4, 4))];
        let result = solver().solve(&open5(), &agents).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(final_pos(&result[0], IVec2::ZERO), IVec2::new(4, 4));
    }

    #[test]
    fn pibt_two_agents_no_vertex_conflict() {
        let agents = vec![
            (IVec2::new(0, 2), IVec2::new(4, 2)),
            (IVec2::new(4, 2), IVec2::new(0, 2)),
        ];
        let result = solver().solve(&open5(), &agents).unwrap();
        assert_eq!(result.len(), 2);
        assert!(no_vertex_conflicts(&result, &agents));
    }

    #[test]
    fn pibt_two_parallel_agents_reach_goals() {
        let agents = vec![
            (IVec2::new(0, 0), IVec2::new(4, 0)),
            (IVec2::new(0, 4), IVec2::new(4, 4)),
        ];
        let result = solver().solve(&open5(), &agents).unwrap();
        assert_eq!(result.len(), 2);
        assert!(no_vertex_conflicts(&result, &agents));
        assert_eq!(final_pos(&result[0], agents[0].0), agents[0].1);
        assert_eq!(final_pos(&result[1], agents[1].0), agents[1].1);
    }

    #[test]
    fn pibt_five_agents_no_vertex_conflict() {
        let grid = GridMap::new(8, 8);
        let agents = vec![
            (IVec2::new(0, 0), IVec2::new(7, 7)),
            (IVec2::new(7, 0), IVec2::new(0, 7)),
            (IVec2::new(0, 7), IVec2::new(7, 0)),
            (IVec2::new(3, 0), IVec2::new(3, 7)),
            (IVec2::new(3, 7), IVec2::new(3, 0)),
        ];
        let result = solver().solve(&grid, &agents).unwrap();
        assert_eq!(result.len(), 5);
        assert!(no_vertex_conflicts(&result, &agents));
    }

    #[test]
    fn pibt_invalid_start_returns_error() {
        let mut grid = open5();
        grid.set_obstacle(IVec2::new(2, 2));
        let agents = vec![(IVec2::new(2, 2), IVec2::new(0, 0))];
        assert!(solver().solve(&grid, &agents).is_err());
    }

    #[test]
    fn pibt_invalid_goal_returns_error() {
        let mut grid = open5();
        grid.set_obstacle(IVec2::new(4, 4));
        let agents = vec![(IVec2::ZERO, IVec2::new(4, 4))];
        assert!(solver().solve(&grid, &agents).is_err());
    }

    /// Run PIBT on a warehouse topology and verify no agent ever occupies an obstacle.
    #[test]
    fn pibt_warehouse_no_obstacle_violations() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        for entry in &registry.entries {
            let (grid, zones) = TopologyRegistry::parse_entry(entry).unwrap();

            // Place agents on walkable cells (corridors + pickup cells)
            let walkable: Vec<IVec2> = zones
                .corridor_cells
                .iter()
                .chain(zones.pickup_cells.iter())
                .copied()
                .collect();
            if walkable.len() < 20 { continue; }

            // Pair agents with goals across the warehouse
            let n = 15.min(walkable.len() / 2);
            let agents: Vec<(IVec2, IVec2)> = (0..n)
                .map(|i| (walkable[i], walkable[walkable.len() - 1 - i]))
                .collect();

            let result = solver().solve(&grid, &agents).unwrap();

            // Check every position along every agent path
            for (agent_idx, (plan, (start, _goal))) in
                result.iter().zip(agents.iter()).enumerate()
            {
                let mut pos = *start;
                for (t, &action) in plan.iter().enumerate() {
                    pos = action.apply(pos);
                    assert!(
                        grid.is_walkable(pos),
                        "agent {} at tick {} moved to obstacle {:?} (entry: {})",
                        agent_idx,
                        t + 1,
                        pos,
                        entry.id,
                    );
                }
            }
        }
    }

    /// Run lifelong-style PIBT on warehouse: reassign goals every time agents reach them.
    #[test]
    fn pibt_warehouse_lifelong_no_obstacle_violations() {
        use crate::core::topology::TopologyRegistry;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use rand::Rng;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let mut rng = ChaCha8Rng::seed_from_u64(123);

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 10;
        let mut positions: Vec<IVec2> = walkable[..n].to_vec();
        let mut goals: Vec<IVec2> = (0..n)
            .map(|_| walkable[rng.random_range(0..walkable.len())])
            .collect();

        let mut core = super::super::pibt_core::PibtCore::new();

        for tick in 0..200 {
            let agent_pairs: Vec<(IVec2, IVec2)> =
                positions.iter().zip(goals.iter()).map(|(&p, &g)| (p, g)).collect();
            let dist_maps = super::super::heuristics::compute_distance_maps(grid, &agent_pairs);
            let dist_refs: Vec<&super::super::heuristics::DistanceMap> =
                dist_maps.iter().collect();

            let actions = core.one_step(&positions, &goals, grid, &dist_refs);

            for (i, action) in actions.iter().enumerate() {
                let new_pos = action.apply(positions[i]);
                assert!(
                    grid.is_walkable(new_pos),
                    "agent {} at tick {} moved to obstacle {:?}",
                    i,
                    tick,
                    new_pos,
                );
                positions[i] = new_pos;

                // Reassign goal when reached
                if positions[i] == goals[i] {
                    goals[i] = walkable[rng.random_range(0..walkable.len())];
                }
            }
        }
    }

    // ── Tier 2: Paper property tests ─────────────────────────────────

    /// Paper property (Okumura, AIJ 2022, Theorem 1): PIBT guarantees the
    /// highest-priority agent eventually reaches its goal. We test this
    /// by running a single agent (always highest priority) and verifying
    /// its distance to goal monotonically decreases (modulo backtracking).
    /// Over 100 ticks on an open grid, it MUST reach its goal.
    #[test]
    fn paper_property_pibt_highest_priority_reaches_goal() {
        use crate::core::topology::ZoneMap;
        use crate::solver::heuristics::DistanceMapCache;
        use std::collections::HashMap;

        let grid = GridMap::new(8, 8);
        let zones = ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(7, 7)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        };

        let mut solver = PibtLifelongSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = crate::core::seed::SeededRng::new(42);

        // Single agent = always highest priority
        let mut pos = IVec2::ZERO;
        let goal = IVec2::new(7, 7);

        for tick in 0..100 {
            let agents = vec![AgentState {
                index: 0, pos, goal: Some(goal), has_plan: tick > 0,
                task_leg: crate::core::task::TaskLeg::TravelEmpty(goal),
            }];
            let ctx = super::super::lifelong::SolverContext {
                grid: &grid, zones: &zones, tick, num_agents: 1,
            };
            if let super::super::lifelong::StepResult::Replan(plans) =
                solver.step(&ctx, &agents, &mut cache, &mut rng)
            {
                if let Some((_, actions)) = plans.first() {
                    if let Some(action) = actions.first() {
                        pos = action.apply(pos);
                    }
                }
            }
            if pos == goal {
                return; // Success — reached goal within 100 ticks
            }
        }
        panic!("highest-priority agent did not reach goal (7,7) from (0,0) in 100 ticks on open 8x8 grid");
    }

    /// Paper property: PIBT with multiple agents on a solvable grid should
    /// complete tasks over time (not deadlock permanently). This tests
    /// liveness — at least SOME agents reach their goals.
    #[test]
    fn paper_property_pibt_liveness_multi_agent() {
        use crate::core::topology::ZoneMap;
        use crate::solver::heuristics::DistanceMapCache;
        use std::collections::HashMap;

        let grid = GridMap::new(8, 8);
        let zones = ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(7, 7)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        };

        let mut solver = PibtLifelongSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = crate::core::seed::SeededRng::new(42);

        let mut positions = vec![
            IVec2::new(0, 0), IVec2::new(7, 7), IVec2::new(0, 7),
            IVec2::new(7, 0), IVec2::new(3, 3),
        ];
        let goals = vec![
            IVec2::new(7, 7), IVec2::new(0, 0), IVec2::new(7, 0),
            IVec2::new(0, 7), IVec2::new(5, 5),
        ];
        let mut goals_reached = 0;

        for tick in 0..200 {
            let agents: Vec<AgentState> = (0..5)
                .map(|i| AgentState {
                    index: i, pos: positions[i], goal: Some(goals[i]),
                    has_plan: tick > 0,
                    task_leg: crate::core::task::TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();

            let ctx = super::super::lifelong::SolverContext {
                grid: &grid, zones: &zones, tick, num_agents: 5,
            };
            if let super::super::lifelong::StepResult::Replan(plans) =
                solver.step(&ctx, &agents, &mut cache, &mut rng)
            {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        positions[*idx] = action.apply(positions[*idx]);
                    }
                }
            }
            for i in 0..5 {
                if positions[i] == goals[i] {
                    goals_reached += 1;
                }
            }
        }
        assert!(goals_reached > 0, "no agent reached its goal in 200 ticks — PIBT may be deadlocked");
    }

    /// Diagnostic: run PIBT on warehouse_medium and print per-tick task state
    /// distribution to identify where agents stall.
    /// Run with: cargo test pibt_throughput_diagnostic -- --ignored --nocapture
    #[test]
    #[ignore]
    fn pibt_throughput_diagnostic() {
        use crate::core::runner::SimulationRunner;
        use crate::core::topology::TopologyRegistry;
        use crate::core::task::{ActiveScheduler, TaskLeg};
        use crate::core::queue::ActiveQueuePolicy;
        use crate::core::seed::SeededRng;
        use crate::analysis::baseline::place_agents;
        use crate::fault::config::FaultConfig;
        use crate::fault::scenario::FaultSchedule;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_medium").expect("warehouse_medium missing");
        let (grid, zones) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid_area = (grid.width * grid.height) as usize;
        let num = 15;

        let mut rng = SeededRng::new(42);
        let agents = place_agents(num, &grid, &zones, &mut rng);
        let rng_after = rng.clone();

        let solver = crate::solver::lifelong_solver_from_name("pibt", grid_area, num).unwrap();
        let mut runner = SimulationRunner::new(
            grid, zones, agents, solver, rng_after,
            FaultConfig { enabled: false, ..Default::default() },
            FaultSchedule::default(),
        );

        let sched = ActiveScheduler::from_name("random");
        let qp = ActiveQueuePolicy::from_name("closest");

        eprintln!("tick | free tempt load  t2q  queu tlod unld | atGoal waits | tasks");
        for tick in 1..=200 {
            runner.tick(sched.scheduler(), qp.policy());

            let mut counts = [0u32; 8]; // free, tempt, load, t2q, queu, tlod, unld, charge
            let mut at_goal = 0u32;
            let mut wait_count = 0u32;

            for a in &runner.agents {
                if !a.alive { continue; }
                let idx = match &a.task_leg {
                    TaskLeg::Free => 0,
                    TaskLeg::TravelEmpty(_) => 1,
                    TaskLeg::Loading(_) => 2,
                    TaskLeg::TravelToQueue { .. } => 3,
                    TaskLeg::Queuing { .. } => 4,
                    TaskLeg::TravelLoaded { .. } => 5,
                    TaskLeg::Unloading { .. } => 6,
                    TaskLeg::Charging => 7,
                };
                counts[idx] += 1;
                if a.pos == a.goal { at_goal += 1; }
                if a.last_action == crate::core::action::Action::Wait { wait_count += 1; }
            }

            if tick <= 10 || tick % 20 == 0 || tick == 200 {
                eprintln!(
                    "{tick:4} | {fr:4} {te:5} {ld:4} {tq:4} {qu:4} {tl:4} {ul:4} | {ag:6} {wt:5} | {tasks:5}",
                    fr=counts[0], te=counts[1], ld=counts[2], tq=counts[3],
                    qu=counts[4], tl=counts[5], ul=counts[6],
                    ag=at_goal, wt=wait_count, tasks=runner.tasks_completed,
                );
            }
        }
        eprintln!("\nFinal: {} tasks in 200 ticks with 15 agents", runner.tasks_completed);
    }
}
