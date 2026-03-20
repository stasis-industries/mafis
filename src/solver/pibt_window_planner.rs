//! PIBT-Window planner for RHCR — unrolls PIBT for H steps.
//!
//! Unlike standalone PIBT (1-step), this simulates H timesteps ahead in one
//! call, producing longer plans. Cooperative, fast, suboptimal.

use bevy::prelude::*;

use crate::core::action::Action;
use crate::core::seed::SeededRng;

use super::pibt_core::PibtCore;
use super::windowed::{PlanFragment, WindowContext, WindowResult, WindowedPlanner};

pub struct PibtWindowPlanner {
    core: PibtCore,
}

impl Default for PibtWindowPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PibtWindowPlanner {
    pub fn new() -> Self {
        Self {
            core: PibtCore::new(),
        }
    }
}

impl WindowedPlanner for PibtWindowPlanner {
    fn name(&self) -> &'static str {
        "pibt_window"
    }

    fn plan_window(
        &mut self,
        ctx: &WindowContext,
        _rng: &mut SeededRng,
    ) -> WindowResult {
        let n = ctx.agents.len();
        if n == 0 {
            return WindowResult::Solved(Vec::new());
        }

        let mut positions: Vec<IVec2> = ctx.agents.iter().map(|a| a.pos).collect();
        let goals: Vec<IVec2> = ctx.agents.iter().map(|a| a.goal).collect();
        let mut plans: Vec<Vec<Action>> = vec![Vec::with_capacity(ctx.horizon); n];

        // Reset core priorities for fresh window
        self.core.reset();

        // Unroll PIBT for H steps
        for _t in 0..ctx.horizon {
            // Check if all agents reached goals
            if positions.iter().zip(goals.iter()).all(|(p, g)| p == g) {
                break;
            }

            let actions = self.core.one_step(
                &positions,
                &goals,
                ctx.grid,
                ctx.distance_maps,
            );

            for i in 0..n {
                let action = actions[i];
                positions[i] = action.apply(positions[i]);
                plans[i].push(action);
            }
        }

        // Detect stuck agents (sat at same cell entire window)
        let mut failed = Vec::new();
        let mut solved = Vec::new();

        for (i, plan) in plans.into_iter().enumerate() {
            let stuck = !plan.is_empty()
                && positions[i] != goals[i]
                && plan.iter().all(|a| *a == Action::Wait);

            if stuck {
                failed.push(ctx.agents[i].index);
            } else {
                solved.push(PlanFragment {
                    agent_index: ctx.agents[i].index,
                    actions: plan.into_iter().collect(),
                });
            }
        }

        if failed.is_empty() {
            WindowResult::Solved(solved)
        } else {
            WindowResult::Partial { solved, failed }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::solver::heuristics::DistanceMap;
    use crate::solver::windowed::WindowAgent;

    #[test]
    fn pibt_window_single_agent() {
        let grid = GridMap::new(5, 5);
        let agents = vec![WindowAgent { index: 0, pos: IVec2::ZERO, goal: IVec2::new(4, 4), goal_sequence: SmallVec::new() }];
        let dm = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm];
        let ctx = super::super::windowed::WindowContext {
            grid: &grid,
            horizon: 20,
            node_limit: 0,
            agents: &agents,
            distance_maps: &dist_maps,
        };
        let mut planner = PibtWindowPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        match result {
            WindowResult::Solved(frags) => {
                assert_eq!(frags.len(), 1);
                // Should plan 8 steps (manhattan distance 8 on open grid)
                assert_eq!(frags[0].actions.len(), 8);
            }
            _ => panic!("expected Solved"),
        }
    }
}
