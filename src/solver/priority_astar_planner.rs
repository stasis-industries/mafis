//! Priority A* planner for RHCR — sequential spacetime A* with priority ordering.
//!
//! Agents are planned one at a time in priority order (closest-to-goal first).
//! Each agent's path becomes a constraint for subsequent agents. No backtracking
//! (unlike PBS), so it's faster but can fail under heavy congestion.

use bevy::prelude::*;

use crate::core::action::Action;
use crate::core::seed::SeededRng;

use super::astar::{FlatConstraintIndex, SpacetimeGrid, spacetime_astar_fast};
use super::windowed::{PlanFragment, WindowContext, WindowResult, WindowedPlanner};

pub struct PriorityAStarPlanner {
    ci: FlatConstraintIndex,
    stg: SpacetimeGrid,
}

impl Default for PriorityAStarPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PriorityAStarPlanner {
    pub fn new() -> Self {
        Self {
            ci: FlatConstraintIndex::new(1, 1, 1),
            stg: SpacetimeGrid::new(),
        }
    }
}

impl WindowedPlanner for PriorityAStarPlanner {
    fn name(&self) -> &'static str {
        "priority_astar"
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

        // Sort agents by distance to goal (ascending — closest plans first).
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_unstable_by_key(|&i| {
            let dm = ctx.distance_maps[i];
            dm.get(ctx.agents[i].pos)
        });

        let mut all_plans: Vec<Option<Vec<Action>>> = vec![None; n];
        // Flat constraint index — zero-hashing O(1) lookups
        self.ci.reset(ctx.grid.width, ctx.grid.height, ctx.horizon as u64);
        let mut failed = Vec::new();

        for &i in &order {
            let agent = &ctx.agents[i];

            match spacetime_astar_fast(
                ctx.grid,
                agent.pos,
                agent.goal,
                &self.ci,
                ctx.horizon as u64,
                Some(ctx.distance_maps[i]),
                &mut self.stg,
                u64::MAX,
            ) {
                Ok(plan) => {
                    add_plan_to_flat_index(&mut self.ci, &plan, agent.pos, ctx.horizon);
                    all_plans[i] = Some(plan);
                }
                Err(_) => {
                    failed.push(agent.index);
                }
            }
        }

        let solved: Vec<PlanFragment> = all_plans
            .into_iter()
            .zip(ctx.agents.iter())
            .filter_map(|(plan, agent)| {
                plan.map(|p| PlanFragment {
                    agent_index: agent.index,
                    actions: p.into_iter().collect(),
                })
            })
            .collect();

        if failed.is_empty() {
            WindowResult::Solved(solved)
        } else {
            WindowResult::Partial { solved, failed }
        }
    }
}

/// Add a planned agent's trajectory to the flat constraint index (incremental).
fn add_plan_to_flat_index(
    ci: &mut FlatConstraintIndex,
    plan: &[Action],
    start: IVec2,
    horizon: usize,
) {
    let mut pos = start;
    for (t, &action) in plan.iter().enumerate() {
        let next_pos = action.apply(pos);
        ci.add_vertex(next_pos, (t + 1) as u64);
        ci.add_edge(next_pos, pos, t as u64);
        pos = next_pos;
    }
    // After plan ends, agent stays at final position
    let final_t = plan.len();
    for t in final_t..(horizon + 1) {
        ci.add_vertex(pos, t as u64);
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
    fn priority_astar_single_agent() {
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
        let mut planner = PriorityAStarPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        match result {
            WindowResult::Solved(frags) => {
                assert_eq!(frags.len(), 1);
                assert_eq!(frags[0].actions.len(), 8);
            }
            _ => panic!("expected Solved"),
        }
    }

    #[test]
    fn priority_astar_two_parallel_agents() {
        let grid = GridMap::new(5, 5);
        let agents = vec![
            WindowAgent { index: 0, pos: IVec2::new(0, 0), goal: IVec2::new(4, 0), goal_sequence: SmallVec::new() },
            WindowAgent { index: 1, pos: IVec2::new(0, 4), goal: IVec2::new(4, 4), goal_sequence: SmallVec::new() },
        ];
        let dm0 = DistanceMap::compute(&grid, IVec2::new(4, 0));
        let dm1 = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm0, &dm1];
        let ctx = super::super::windowed::WindowContext {
            grid: &grid,
            horizon: 20,
            node_limit: 0,
            agents: &agents,
            distance_maps: &dist_maps,
        };
        let mut planner = PriorityAStarPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        assert!(matches!(result, WindowResult::Solved(_)));
    }
}
