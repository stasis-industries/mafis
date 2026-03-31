//! RHCR — Rolling-Horizon Collision Resolution
//!
//! Windowed lifelong solver that replans every W ticks for a horizon of H steps.
//! Configurable conflict resolution mode: PBS, PIBT-Window, or Priority A*.
//! Falls back to PIBT when the windowed planner fails.
//!
//! Reference: Li et al., "Lifelong Multi-Agent Path Finding in Large-Scale
//! Warehouses" (AAAI 2021).

use bevy::prelude::*;
use smallvec::smallvec;

use crate::constants;
use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::heuristics::DistanceMapCache;
use super::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use super::pibt_core::PibtCore;
use super::traits::{Optimality, Scalability, SolverInfo};
use super::windowed::{WindowAgent, WindowContext, WindowResult, WindowedPlanner};

use super::pbs_planner::PbsPlanner;
use super::pibt_window_planner::PibtWindowPlanner;
use super::priority_astar_planner::PriorityAStarPlanner;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RhcrMode {
    Pbs,
    PibtWindow,
    PriorityAStar,
}

impl RhcrMode {
    pub fn label(&self) -> &'static str {
        match self {
            RhcrMode::Pbs => "PBS",
            RhcrMode::PibtWindow => "PIBT-Window",
            RhcrMode::PriorityAStar => "Priority A*",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            RhcrMode::Pbs => "rhcr_pbs",
            RhcrMode::PibtWindow => "rhcr_pibt",
            RhcrMode::PriorityAStar => "rhcr_priority_astar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackMode {
    /// Keep solved agents' plans, PIBT only the failed ones.
    PerAgent,
    /// Discard all, PIBT everyone.
    Full,
    /// Keep partial plans up to tick budget, PIBT the rest.
    Tiered,
}

impl FallbackMode {
    pub fn label(&self) -> &'static str {
        match self {
            FallbackMode::PerAgent => "Per-Agent",
            FallbackMode::Full => "Full",
            FallbackMode::Tiered => "Tiered",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            FallbackMode::PerAgent => "per_agent",
            FallbackMode::Full => "full",
            FallbackMode::Tiered => "tiered",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RhcrConfig {
    pub mode: RhcrMode,
    pub fallback: FallbackMode,
    pub horizon: usize,
    pub replan_interval: usize,
    pub pbs_node_limit: usize,
}

impl RhcrConfig {
    /// Compute smart defaults from grid dimensions and agent count.
    ///
    /// Key trade-off: longer horizons = better plans, but larger frame stalls.
    /// We cap H based on mode cost to keep worst-case replan under ~50ms in WASM.
    pub fn auto(mode: RhcrMode, grid_area: usize, num_agents: usize) -> Self {
        let density = if grid_area > 0 {
            num_agents as f32 / grid_area as f32
        } else {
            0.0
        };

        // Mode-dependent max horizon: expensive modes get shorter horizons
        // to keep per-replan cost bounded for WASM frame budget.
        let mode_max_h = match mode {
            RhcrMode::PibtWindow => constants::RHCR_MAX_HORIZON, // PIBT is fast
            RhcrMode::PriorityAStar => 20, // A* scales with n × H
            RhcrMode::Pbs => 15,           // PBS tree search is exponential
        };

        // H: scale with grid size, but cap by density and mode
        let base_h = (grid_area as f32).sqrt().ceil() as usize;
        let h = if density > 0.15 {
            base_h.min(10)
        } else if density > 0.05 {
            base_h.min(15)
        } else {
            base_h.min(mode_max_h)
        };
        let h = h.clamp(
            constants::RHCR_MIN_HORIZON,
            constants::RHCR_MAX_HORIZON,
        );

        // W: replan more often to spread cost across ticks.
        // W = H/3 keeps plans fresh without huge per-replan stalls.
        let w = if density > 0.15 {
            // High density: replan more aggressively (W = H/4)
            (h / 4).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else if density > 0.05 {
            // Medium density: standard interval (W = H/3)
            (h / 3).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else {
            // Low density: replan less often — agents have more room to follow stale plans
            (h / 2).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        };

        let fallback = match mode {
            RhcrMode::Pbs => FallbackMode::PerAgent,
            RhcrMode::PibtWindow => FallbackMode::Full,
            RhcrMode::PriorityAStar => FallbackMode::Tiered,
        };

        // Tighter node limit for PBS — reduces worst-case tree explosion
        let pbs_node_limit = (num_agents * 3).clamp(50, constants::PBS_MAX_NODE_LIMIT);

        Self {
            mode,
            fallback,
            horizon: h,
            replan_interval: w,
            pbs_node_limit,
        }
    }
}

// ---------------------------------------------------------------------------
// RHCR Solver
// ---------------------------------------------------------------------------

pub struct RhcrSolver {
    config: RhcrConfig,
    planner: Box<dyn WindowedPlanner>,
    pibt_fallback: PibtCore,
    ticks_since_replan: usize,
    plan_buffer: Vec<AgentPlan>,
    /// Previous positions for congestion detection (reference C++ BasicSystem::congested).
    prev_positions: Vec<IVec2>,
    /// Consecutive ticks where >50% of agents are stuck.
    congestion_streak: usize,
}

impl RhcrSolver {
    pub fn new(config: RhcrConfig) -> Self {
        let planner: Box<dyn WindowedPlanner> = match config.mode {
            RhcrMode::Pbs => Box::new(PbsPlanner::new()),
            RhcrMode::PibtWindow => Box::new(PibtWindowPlanner::new()),
            RhcrMode::PriorityAStar => Box::new(PriorityAStarPlanner::new()),
        };

        // Start at replan_interval - 1 so the very first step() triggers a replan.
        // Without this, agents would sit idle for W ticks before getting any plans.
        let initial_counter = config.replan_interval.saturating_sub(1);

        Self {
            config,
            planner,
            pibt_fallback: PibtCore::new(),
            ticks_since_replan: initial_counter,
            plan_buffer: Vec::new(),
            prev_positions: Vec::new(),
            congestion_streak: 0,
        }
    }

    pub fn config(&self) -> &RhcrConfig {
        &self.config
    }

    /// Check congestion: returns true if >50% of agents didn't move since last tick.
    /// Reference: BasicSystem::congested() in the RHCR C++ codebase.
    fn update_congestion(&mut self, agents: &[AgentState]) -> bool {
        if agents.is_empty() {
            self.congestion_streak = 0;
            return false;
        }

        let n = agents.len();
        if self.prev_positions.len() != n {
            // First call or agent count changed — initialize
            self.prev_positions = agents.iter().map(|a| a.pos).collect();
            self.congestion_streak = 0;
            return false;
        }

        let stuck_count = agents
            .iter()
            .enumerate()
            .filter(|(i, a)| a.pos == self.prev_positions[*i] && a.goal.is_some() && a.pos != a.goal.unwrap())
            .count();

        // Update positions for next tick
        for (i, a) in agents.iter().enumerate() {
            self.prev_positions[i] = a.pos;
        }

        let congested = stuck_count * 2 > n; // >50% stuck
        if congested {
            self.congestion_streak += 1;
        } else {
            self.congestion_streak = 0;
        }
        congested
    }

    /// Effective replan interval, shortened during congestion.
    /// When congested, replan every 2 ticks (minimum) to break deadlocks faster.
    fn effective_replan_interval(&self) -> usize {
        if self.congestion_streak >= 2 {
            // Congested for 2+ ticks: replan aggressively
            constants::RHCR_MIN_REPLAN_INTERVAL
        } else {
            self.config.replan_interval
        }
    }

    /// Apply PIBT fallback to a set of agent indices.
    /// `solved_first_steps` contains (agent_index, next_step_target) for
    /// agents whose plans were kept — PIBT must not collide with them.
    /// `solved_current_positions` contains the current cell of each solved agent —
    /// included so PIBT's occupation grid doesn't ignore them as "blockers".
    fn pibt_fallback_for(
        &mut self,
        failed_indices: &[usize],
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        grid: &GridMap,
        solved_first_steps: &[(usize, IVec2)],
    ) {
        if failed_indices.is_empty() {
            return;
        }

        // Build sub-problem for failed agents only
        let sub_agents: Vec<&AgentState> = failed_indices
            .iter()
            .filter_map(|&idx| agents.iter().find(|a| a.index == idx))
            .collect();

        if sub_agents.is_empty() {
            return;
        }

        // Build combined positions for PIBT: failed agents first, then solved agents as "dummy"
        // agents that stay in place. This makes PibtCore's occupation grid aware of solved
        // agents' current cells, preventing failed agents from moving into them.
        let solved_current: Vec<IVec2> = solved_first_steps.iter()
            .filter_map(|&(idx, _)| agents.iter().find(|a| a.index == idx).map(|a| a.pos))
            .collect();

        let mut all_positions: Vec<IVec2> = sub_agents.iter().map(|a| a.pos).collect();
        let mut all_goals: Vec<IVec2> = sub_agents.iter().map(|a| a.goal.unwrap_or(a.pos)).collect();

        // Dummy solved agents: goal == current pos so they always plan Wait
        all_positions.extend_from_slice(&solved_current);
        all_goals.extend_from_slice(&solved_current);

        // Single cache call for all agents (failed + dummy solved)
        let all_pairs: Vec<(IVec2, IVec2)> = all_positions.iter().zip(all_goals.iter()).map(|(&p, &g)| (p, g)).collect();
        let all_dist_maps = distance_cache.get_or_compute(grid, &all_pairs);

        let n_failed = sub_agents.len();
        let actions = self.pibt_fallback.one_step(&all_positions, &all_goals, grid, &all_dist_maps);

        // Collect next-step targets of solved agents (first step or current pos if empty plan)
        let solved_targets: std::collections::HashSet<IVec2> =
            solved_first_steps.iter().map(|&(_, t)| t).collect();

        // Only apply the first n_failed actions (ignore dummy solved agents)
        for (i, &action) in actions.iter().take(n_failed).enumerate() {
            let target = action.apply(sub_agents[i].pos);
            if solved_targets.contains(&target) {
                // Collision with solved agent's next position — force wait
                self.plan_buffer
                    .push((sub_agents[i].index, smallvec![Action::Wait]));
            } else {
                self.plan_buffer
                    .push((sub_agents[i].index, smallvec![action]));
            }
        }
    }
}

impl LifelongSolver for RhcrSolver {
    fn name(&self) -> &'static str {
        match self.config.mode {
            RhcrMode::Pbs => "rhcr_pbs",
            RhcrMode::PibtWindow => "rhcr_pibt",
            RhcrMode::PriorityAStar => "rhcr_priority_astar",
        }
    }

    fn info(&self) -> SolverInfo {
        let (desc, scalability) = match self.config.mode {
            RhcrMode::Pbs => (
                "RHCR (PBS) — windowed planning with Priority-Based Search. High quality, bounded by node limit.",
                Scalability::Medium,
            ),
            RhcrMode::PibtWindow => (
                "RHCR (PIBT-Window) — windowed PIBT unrolled for H steps. Fast and cooperative.",
                Scalability::High,
            ),
            RhcrMode::PriorityAStar => (
                "RHCR (Priority A*) — sequential spacetime A* with priority ordering. Good for moderate density.",
                Scalability::Medium,
            ),
        };

        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(W⁻¹ × mode_cost) amortized per tick",
            scalability,
            description: desc,
            recommended_max_agents: match self.config.mode {
                RhcrMode::Pbs => Some(200),
                RhcrMode::PibtWindow => None,
                RhcrMode::PriorityAStar => Some(300),
            },
        }
    }

    fn reset(&mut self) {
        self.ticks_since_replan = self.config.replan_interval.saturating_sub(1);
        self.pibt_fallback.reset();
        self.plan_buffer.clear();
        self.prev_positions.clear();
        self.congestion_streak = 0;
    }

    fn save_priorities(&self) -> Vec<f32> {
        self.pibt_fallback.priorities().to_vec()
    }

    fn restore_priorities(&mut self, priorities: &[f32]) {
        self.pibt_fallback.set_priorities(priorities);
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        self.ticks_since_replan += 1;

        // Sync PIBT fallback shuffle seed with tick for deterministic tie-breaking after rewind.
        self.pibt_fallback.set_shuffle_seed(ctx.tick);

        // Update congestion detection (reference C++ BasicSystem::congested)
        self.update_congestion(agents);

        // Force replan when any agent has no plan (goal changed mid-window).
        // Require a minimum cooldown of 2 ticks to avoid replanning every tick
        // when goals change rapidly (e.g., high-throughput warehouses).
        let force_replan = self.ticks_since_replan >= 2
            && agents.iter().any(|a| !a.has_plan && a.goal.is_some() && a.pos != a.goal.unwrap());

        // Dynamic replan interval: shorten during congestion to break deadlocks
        let effective_interval = self.effective_replan_interval();

        // Not time to replan yet — zero-cost path
        if !force_replan && self.ticks_since_replan < effective_interval {
            return StepResult::Continue;
        }

        // Time to replan
        self.ticks_since_replan = 0;
        self.plan_buffer.clear();

        if agents.is_empty() {
            return StepResult::Replan(&self.plan_buffer);
        }

        // Build WindowAgent list
        let window_agents: Vec<WindowAgent> = agents
            .iter()
            .map(|a| WindowAgent {
                index: a.index,
                pos: a.pos,
                goal: a.goal.unwrap_or(a.pos),
                goal_sequence: smallvec::SmallVec::new(),
            })
            .collect();

        // Build distance maps
        let pairs: Vec<(IVec2, IVec2)> = window_agents
            .iter()
            .map(|a| (a.pos, a.goal))
            .collect();
        let dist_maps = distance_cache.get_or_compute(ctx.grid, &pairs);

        let window_ctx = WindowContext {
            grid: ctx.grid,
            horizon: self.config.horizon,
            node_limit: self.config.pbs_node_limit,
            agents: &window_agents,
            distance_maps: &dist_maps,
        };

        // Run windowed planner
        let result = self.planner.plan_window(&window_ctx, rng);

        match result {
            WindowResult::Solved(fragments) => {
                for frag in fragments {
                    self.plan_buffer.push((frag.agent_index, frag.actions));
                }
            }
            WindowResult::Partial { solved, failed } => {
                // Compute first-step targets for solved agents (used by PIBT fallback).
                // Agents with empty plans are already at their goal and stay in place —
                // include their current position so the fallback doesn't move into it.
                let solved_first_steps: Vec<(usize, IVec2)> = solved
                    .iter()
                    .filter_map(|frag| {
                        let agent = agents.iter().find(|a| a.index == frag.agent_index)?;
                        let target = frag.actions.first()
                            .map(|action| action.apply(agent.pos))
                            .unwrap_or(agent.pos); // empty plan → stays in place
                        Some((frag.agent_index, target))
                    })
                    .collect();

                match self.config.fallback {
                    FallbackMode::PerAgent => {
                        // Keep solved plans
                        for frag in solved {
                            self.plan_buffer.push((frag.agent_index, frag.actions));
                        }
                        // PIBT fallback for failed (aware of solved agents' first steps)
                        self.pibt_fallback_for(
                            &failed, agents, distance_cache, ctx.grid, &solved_first_steps,
                        );
                    }
                    FallbackMode::Full => {
                        // Discard all windowed plans, PIBT everyone
                        let all_indices: Vec<usize> = agents.iter().map(|a| a.index).collect();
                        self.pibt_fallback_for(
                            &all_indices, agents, distance_cache, ctx.grid, &[],
                        );
                    }
                    FallbackMode::Tiered => {
                        // Keep solved plans, PIBT the rest
                        for frag in solved {
                            self.plan_buffer.push((frag.agent_index, frag.actions));
                        }
                        self.pibt_fallback_for(
                            &failed, agents, distance_cache, ctx.grid, &solved_first_steps,
                        );
                    }
                }
            }
        }

        StepResult::Replan(&self.plan_buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::task::TaskLeg;
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
    fn rhcr_first_call_replans_then_continues() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PibtWindow,
            fallback: FallbackMode::Full,
            horizon: 10,
            replan_interval: 5,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];

        let ctx = SolverContext {
            grid: &grid,
            zones: &zones,
            tick: 0,
            num_agents: 1,
        };

        // First call should replan immediately (counter starts at W-1)
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));

        // After replan, agent has a plan — simulate ECS applying it.
        // Move the agent each tick to avoid triggering congestion detection.
        // Next 4 calls should return Continue (ticks 1-4 of new window)
        for tick in 1..=4 {
            let agents_moving = vec![AgentState {
                index: 0,
                pos: IVec2::new(tick, 0),
                goal: Some(IVec2::new(4, 4)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            }];
            let result = solver.step(&ctx, &agents_moving, &mut cache, &mut rng);
            assert!(matches!(result, StepResult::Continue), "tick {} should be Continue", tick);
        }

        // 5th call after replan should replan again
        let agents_at_4 = vec![AgentState {
            index: 0,
            pos: IVec2::new(4, 1),
            goal: Some(IVec2::new(4, 4)),
            has_plan: true,
            task_leg: TaskLeg::Free,
        }];
        let result = solver.step(&ctx, &agents_at_4, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));
    }

    #[test]
    fn rhcr_auto_config_scales() {
        // Small grid, few agents
        let cfg = RhcrConfig::auto(RhcrMode::Pbs, 100, 5);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
        assert!(cfg.replan_interval >= constants::RHCR_MIN_REPLAN_INTERVAL);
        assert!(cfg.replan_interval <= cfg.horizon);

        // Large grid, many agents
        let cfg = RhcrConfig::auto(RhcrMode::Pbs, 16384, 400);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
    }

    #[test]
    fn rhcr_pbs_mode_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::Pbs,
            fallback: FallbackMode::PerAgent,
            horizon: 15,
            replan_interval: 1, // replan every tick for testing
            pbs_node_limit: 500,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![
            AgentState { index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 0)), has_plan: false, task_leg: TaskLeg::Free },
            AgentState { index: 1, pos: IVec2::new(0, 4), goal: Some(IVec2::new(4, 4)), has_plan: false, task_leg: TaskLeg::Free },
        ];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        match result {
            StepResult::Replan(plans) => {
                assert_eq!(plans.len(), 2);
                assert!(!plans[0].1.is_empty());
                assert!(!plans[1].1.is_empty());
            }
            _ => panic!("expected Replan"),
        }
    }

    #[test]
    fn rhcr_priority_astar_mode_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PriorityAStar,
            fallback: FallbackMode::Tiered,
            horizon: 15,
            replan_interval: 1,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![
            AgentState { index: 0, pos: IVec2::new(0, 0), goal: Some(IVec2::new(4, 4)), has_plan: false, task_leg: TaskLeg::Free },
        ];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));
    }

    #[test]
    fn rhcr_force_replans_when_agent_has_no_plan() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PibtWindow,
            fallback: FallbackMode::Full,
            horizon: 10,
            replan_interval: 5,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        // Initial replan
        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));

        // Normal continue (agent has plan, not at goal)
        let agents_with_plan = vec![AgentState {
            index: 0,
            pos: IVec2::new(1, 0),
            goal: Some(IVec2::new(4, 4)),
            has_plan: true,
            task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 4)),
        }];
        let result = solver.step(&ctx, &agents_with_plan, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Continue));

        // Simulate goal change: agent reached old goal, got new goal, plan cleared
        let agents_new_goal = vec![AgentState {
            index: 0,
            pos: IVec2::new(4, 4),
            goal: Some(IVec2::new(0, 0)),
            has_plan: false,
            task_leg: TaskLeg::TravelLoaded { from: IVec2::new(4, 4), to: IVec2::ZERO },
        }];
        // Should force-replan even though W ticks haven't elapsed
        let result = solver.step(&ctx, &agents_new_goal, &mut cache, &mut rng);
        assert!(
            matches!(result, StepResult::Replan(_)),
            "RHCR should force-replan when agent has no plan and hasn't reached goal"
        );
    }

    /// Full loop: RHCR on a warehouse with goal recycling — no obstacle violations.
    /// Also prints timing for perf regression detection.
    #[test]
    fn rhcr_warehouse_lifelong_no_obstacle_violations() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 8;
        let ticks = 100;

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, n, ticks);
        }
    }

    /// Scale benchmark: 50 agents on medium warehouse.
    #[test]
    fn rhcr_scale_50_agents() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells.iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, 50, 50);
        }
    }

    /// Scale benchmark: 80 agents on medium warehouse.
    #[test]
    fn rhcr_scale_80_agents() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells.iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, 80, 50);
        }
    }

    /// Scale benchmark: 30 agents on medium warehouse.
    #[test]
    fn rhcr_scale_benchmark_medium_warehouse() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 30;
        let ticks = 50;

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, n, ticks);
        }
    }

    fn run_rhcr_bench(
        mode: RhcrMode,
        grid: &crate::core::grid::GridMap,
        zones: &ZoneMap,
        walkable: &[IVec2],
        n: usize,
        ticks: u64,
    ) {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use rand::Rng;
        use std::time::Instant;

        let mut rng_seed = ChaCha8Rng::seed_from_u64(99);
        let mut positions: Vec<IVec2> = walkable[..n].to_vec();
        let mut goals: Vec<IVec2> = (0..n)
            .map(|_| walkable[rng_seed.random_range(0..walkable.len())])
            .collect();
        let mut plans: Vec<std::collections::VecDeque<Action>> =
            vec![std::collections::VecDeque::new(); n];

        let config = RhcrConfig::auto(mode, (grid.width * grid.height) as usize, n);
        let h = config.horizon;
        let w = config.replan_interval;
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let start = Instant::now();

        for tick in 0..ticks {
            for i in 0..n {
                let action = plans[i].pop_front().unwrap_or(Action::Wait);
                let new_pos = action.apply(positions[i]);
                assert!(
                    grid.is_walkable(new_pos),
                    "RHCR {:?}: agent {} tick {} moved to obstacle {:?}",
                    mode, i, tick, new_pos,
                );
                positions[i] = new_pos;
            }

            for i in 0..n {
                if positions[i] == goals[i] {
                    goals[i] = walkable[rng_seed.random_range(0..walkable.len())];
                    plans[i].clear();
                }
            }

            let agent_states: Vec<AgentState> = (0..n)
                .map(|i| AgentState {
                    index: i,
                    pos: positions[i],
                    goal: Some(goals[i]),
                    has_plan: !plans[i].is_empty(),
                    task_leg: TaskLeg::Free,
                })
                .collect();

            let solver_zones = ZoneMap {
                pickup_cells: zones.pickup_cells.clone(),
                delivery_cells: zones.delivery_cells.clone(),
                corridor_cells: zones.corridor_cells.clone(),
                recharging_cells: Vec::new(),
                zone_type: zones.zone_type.clone(),
                queue_lines: Vec::new(),
            };
            let ctx = SolverContext {
                grid,
                zones: &solver_zones,
                tick,
                num_agents: n,
            };

            match solver.step(&ctx, &agent_states, &mut cache, &mut rng) {
                StepResult::Continue => {}
                StepResult::Replan(new_plans) => {
                    for (idx, actions) in new_plans {
                        if *idx < n {
                            plans[*idx] = actions.iter().copied().collect();
                        }
                    }
                }
            }
        }

        let elapsed = start.elapsed();
        let us_per_tick = elapsed.as_micros() / ticks as u128;
        eprintln!(
            "RHCR {:?}: {}ticks × {}agents in {:?} ({} µs/tick) [H={} W={}]",
            mode, ticks, n, elapsed, us_per_tick, h, w,
        );
    }
}
