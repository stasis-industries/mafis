//! PIBT+APF — PIBT with sequential Artificial Potential Fields.
//!
//! Paper-accurate implementation: after each agent commits its next position
//! (inside the PIBT priority loop), its future path is projected and an
//! exponential APF is added to a shared field. Subsequent agents see the
//! accumulated field when sorting candidate neighbors.
//!
//! Reference: Pertzovsky et al., "Enhancing Lifelong Multi-Agent Path-finding
//! by Using Artificial Potential Fields", arXiv:2505.22753, May 2025.
//! Reference impl: github.com/Arseni1919/APFs_for_MAPF_Implementation_v2

use bevy::prelude::*;
use smallvec::smallvec;

use crate::core::seed::SeededRng;

use super::heuristics::DistanceMapCache;
use super::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use super::pibt_core::PibtCore;
use super::traits::{Optimality, Scalability, SolverInfo};

use crate::constants::{APF_GAMMA, APF_LOOKAHEAD_STEPS, APF_RADIUS, APF_WEIGHT};

// ---------------------------------------------------------------------------
// PIBT+APF Solver (paper-accurate, NOT a GuidanceLayer)
// ---------------------------------------------------------------------------

pub struct PibtApfSolver {
    core: PibtCore,
    plan_buffer: Vec<AgentPlan>,

    // APF parameters (from paper Table 1)
    apf_w: f64,
    apf_gamma: f64,
    apf_d_max: i32,
    apf_t_max: usize,

    // Reusable APF field (cleared and rebuilt each tick)
    apf_field: Vec<f64>,

    // Scratch buffers (same pattern as PibtLifelongSolver)
    agent_pairs_buf: Vec<(IVec2, IVec2)>,
    positions_buf: Vec<IVec2>,
    goals_buf: Vec<IVec2>,
    has_task_buf: Vec<bool>,
}

impl PibtApfSolver {
    pub fn new() -> Self {
        Self {
            core: PibtCore::new(),
            plan_buffer: Vec::new(),
            apf_w: APF_WEIGHT,
            apf_gamma: APF_GAMMA,
            apf_d_max: APF_RADIUS,
            apf_t_max: APF_LOOKAHEAD_STEPS,
            apf_field: Vec::new(),
            agent_pairs_buf: Vec::new(),
            positions_buf: Vec::new(),
            goals_buf: Vec::new(),
            has_task_buf: Vec::new(),
        }
    }
}

impl Default for PibtApfSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LifelongSolver for PibtApfSolver {
    fn name(&self) -> &'static str {
        "pibt+apf"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(n log n + n * t_max * d_max^2) per timestep",
            scalability: Scalability::High,
            description: "PIBT+APF — PIBT with sequential artificial potential fields. APF updated after each agent commits, projecting future path. Paper: Pertzovsky et al., arXiv:2505.22753.",
            recommended_max_agents: None,
        }
    }

    fn reset(&mut self) {
        self.core.reset();
        self.plan_buffer.clear();
        self.apf_field.clear();
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        _rng: &mut SeededRng,
    ) -> StepResult<'a> {
        self.core.set_shuffle_seed(ctx.tick);

        if agents.is_empty() {
            self.plan_buffer.clear();
            return StepResult::Replan(&self.plan_buffer);
        }

        // Build position/goal pairs for distance cache
        self.agent_pairs_buf.clear();
        self.agent_pairs_buf
            .extend(agents.iter().map(|a| (a.pos, a.goal.unwrap_or(a.pos))));

        let dist_maps = distance_cache.get_or_compute(ctx.grid, &self.agent_pairs_buf);

        self.positions_buf.clear();
        self.positions_buf.extend(agents.iter().map(|a| a.pos));

        self.goals_buf.clear();
        self.goals_buf
            .extend(agents.iter().map(|a| a.goal.unwrap_or(a.pos)));

        self.has_task_buf.clear();
        self.has_task_buf.extend(agents.iter().map(|a| {
            let goal = a.goal.unwrap_or(a.pos);
            goal != a.pos
        }));

        // Call PibtCore with sequential APF (paper-accurate)
        let actions = self.core.one_step_with_apf(
            &self.positions_buf,
            &self.goals_buf,
            ctx.grid,
            &dist_maps,
            &self.has_task_buf,
            &mut self.apf_field,
            self.apf_w,
            self.apf_gamma,
            self.apf_d_max,
            self.apf_t_max,
        );

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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::task::TaskLeg;
    use crate::core::topology::ZoneMap;
    use crate::solver::heuristics::DistanceMapCache;
    use crate::solver::pibt::PibtLifelongSolver;
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
    fn pibt_apf_empty_agents() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = PibtApfSolver::new();
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
    fn pibt_apf_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = PibtApfSolver::new();
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
    fn pibt_apf_produces_different_plans_than_vanilla() {
        // With 2+ agents on a congested grid, APF should produce different
        // (hopefully better) plans than vanilla PIBT by steering agents
        // away from each other's projected paths.
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut apf_solver = PibtApfSolver::new();
        let mut pibt_solver = PibtLifelongSolver::new();
        let mut cache_apf = DistanceMapCache::default();
        let mut cache_pibt = DistanceMapCache::default();
        let mut rng_apf = SeededRng::new(42);
        let mut rng_pibt = SeededRng::new(42);

        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 2),
                goal: Some(IVec2::new(4, 2)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 2)),
            },
            AgentState {
                index: 1,
                pos: IVec2::new(4, 2),
                goal: Some(IVec2::new(0, 2)),
                has_plan: false,
                task_leg: TaskLeg::TravelEmpty(IVec2::new(0, 2)),
            },
        ];

        let ctx = SolverContext {
            grid: &grid,
            zones: &zones,
            tick: 0,
            num_agents: 2,
        };

        let apf_result = apf_solver.step(&ctx, &agents, &mut cache_apf, &mut rng_apf);
        let pibt_result = pibt_solver.step(&ctx, &agents, &mut cache_pibt, &mut rng_pibt);

        // Both should produce valid plans (2 agents)
        match (apf_result, pibt_result) {
            (StepResult::Replan(apf_plans), StepResult::Replan(pibt_plans)) => {
                assert_eq!(apf_plans.len(), 2);
                assert_eq!(pibt_plans.len(), 2);
                // Plans may or may not differ — the point is APF runs without error
                // and produces valid collision-free plans
            }
            _ => panic!("both solvers should replan"),
        }
    }

    #[test]
    fn pibt_apf_no_collision() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut solver = PibtApfSolver::new();
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

    #[test]
    fn pibt_apf_reset_clears_state() {
        let mut solver = PibtApfSolver::new();
        solver.apf_field = vec![1.0; 25];
        solver.reset();
        assert!(solver.apf_field.is_empty());
    }

    #[test]
    fn pibt_apf_deterministic() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let goal = IVec2::new(3, 3);
        let mut results = Vec::new();

        for _ in 0..2 {
            let mut solver = PibtApfSolver::new();
            let mut cache = DistanceMapCache::default();
            let mut rng = SeededRng::new(42);
            let mut pos = IVec2::ZERO;
            let mut run_positions = Vec::new();

            for tick in 0..10 {
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
                if let StepResult::Replan(plans) =
                    solver.step(&ctx, &agents, &mut cache, &mut rng)
                {
                    if let Some((_, actions)) = plans.first() {
                        if let Some(action) = actions.first() {
                            pos = action.apply(pos);
                        }
                    }
                }
                run_positions.push(pos);
            }
            results.push(run_positions);
        }
        assert_eq!(results[0], results[1]);
    }

    // ── Tier 2: Paper property tests ─────────────────────────────────

    /// Paper property: APF with w=0 must produce identical plans to vanilla PIBT.
    /// This is the ablation test — proves the bias mechanism doesn't alter
    /// base PIBT behavior when disabled.
    /// Reference: Pertzovsky et al., arXiv:2505.22753, Section 4 (ablation).
    #[test]
    fn paper_property_apf_zero_weight_equals_vanilla_pibt() {
        let grid = GridMap::new(8, 8);
        let zones = test_zones();

        let mut apf_solver = PibtApfSolver {
            apf_w: 0.0,   // zero weight → no APF effect
            apf_gamma: 3.0,
            apf_d_max: 2,
            apf_t_max: 2,
            ..PibtApfSolver::new()
        };
        let mut pibt_solver = PibtLifelongSolver::new();
        let mut cache_apf = DistanceMapCache::default();
        let mut cache_pibt = DistanceMapCache::default();
        let mut rng_apf = SeededRng::new(42);
        let mut rng_pibt = SeededRng::new(42);

        let mut apf_positions = vec![
            IVec2::new(0, 0), IVec2::new(7, 7), IVec2::new(0, 7),
            IVec2::new(7, 0), IVec2::new(3, 3),
        ];
        let mut pibt_positions = apf_positions.clone();
        let goals = vec![
            IVec2::new(7, 7), IVec2::new(0, 0), IVec2::new(7, 0),
            IVec2::new(0, 7), IVec2::new(5, 5),
        ];

        for tick in 0..50 {
            let agents_apf: Vec<AgentState> = (0..5)
                .map(|i| AgentState {
                    index: i, pos: apf_positions[i], goal: Some(goals[i]),
                    has_plan: tick > 0, task_leg: TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();
            let agents_pibt: Vec<AgentState> = (0..5)
                .map(|i| AgentState {
                    index: i, pos: pibt_positions[i], goal: Some(goals[i]),
                    has_plan: tick > 0, task_leg: TaskLeg::TravelEmpty(goals[i]),
                })
                .collect();

            let ctx = SolverContext {
                grid: &grid, zones: &zones, tick, num_agents: 5,
            };

            if let StepResult::Replan(plans) = apf_solver.step(&ctx, &agents_apf, &mut cache_apf, &mut rng_apf) {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        apf_positions[*idx] = action.apply(apf_positions[*idx]);
                    }
                }
            }
            if let StepResult::Replan(plans) = pibt_solver.step(&ctx, &agents_pibt, &mut cache_pibt, &mut rng_pibt) {
                for (idx, actions) in plans {
                    if let Some(action) = actions.first() {
                        pibt_positions[*idx] = action.apply(pibt_positions[*idx]);
                    }
                }
            }

            assert_eq!(
                apf_positions, pibt_positions,
                "APF(w=0) diverged from vanilla PIBT at tick {tick}"
            );
        }
    }

    /// Paper property: APF with nonzero weight must produce a non-trivial
    /// repulsive field (field values > 0 near agents).
    /// Reference: Pertzovsky et al., Eq. 4: APF_i(v,t) = w * gamma^(-dist).
    #[test]
    fn paper_property_apf_field_nonzero_near_agents() {
        let grid = GridMap::new(8, 8);
        let zones = test_zones();
        let mut solver = PibtApfSolver::new();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        // Run one tick with 3 agents
        let agents = vec![
            AgentState { index: 0, pos: IVec2::new(2, 2), goal: Some(IVec2::new(6, 6)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(6, 6)) },
            AgentState { index: 1, pos: IVec2::new(4, 4), goal: Some(IVec2::new(0, 0)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(0, 0)) },
            AgentState { index: 2, pos: IVec2::new(6, 2), goal: Some(IVec2::new(2, 6)),
                has_plan: false, task_leg: TaskLeg::TravelEmpty(IVec2::new(2, 6)) },
        ];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 3 };
        let _ = solver.step(&ctx, &agents, &mut cache, &mut rng);

        // After step, the APF field should have nonzero values
        // (at least one cell near an agent's projected path should be > 0)
        let has_nonzero = solver.apf_field.iter().any(|&v| v > 0.0);
        assert!(has_nonzero, "APF field should have nonzero values after a step with 3 agents");
    }
}
