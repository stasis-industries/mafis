//! APF Guidance — Artificial Potential Fields for PIBT.
//!
//! Looks several steps ahead along each agent's optimal path and places
//! attractive potential fields around those future positions. Repulsive
//! fields around other agents' predicted positions reduce congestion.
//!
//! Reference: arXiv:2505.22753, May 2025 — "PIBT+APF for Lifelong MAPF"

use bevy::prelude::*;

use crate::core::action::Direction;

use super::guidance::GuidanceLayer;
use super::heuristics::DistanceMapCache;
use super::lifelong::{AgentState, SolverContext};

use crate::constants::{
    APF_ATTRACTIVE_STRENGTH, APF_LOOKAHEAD_STEPS, APF_REPULSIVE_RADIUS, APF_REPULSIVE_STRENGTH,
};

// ---------------------------------------------------------------------------
// APF Guidance
// ---------------------------------------------------------------------------

pub struct ApfGuidance {
    /// Per-agent attractive waypoints (positions along optimal path).
    waypoints: Vec<Vec<IVec2>>,

    /// Flat grid of repulsive field values. Indexed by (y * width + x).
    repulsive_field: Vec<f64>,
    grid_width: i32,

    lookahead: usize,
    attractive_strength: f64,
    repulsive_radius: i32,
    repulsive_strength: f64,
}

impl ApfGuidance {
    pub fn new(_grid_area: usize, _num_agents: usize) -> Self {
        Self {
            waypoints: Vec::new(),
            repulsive_field: Vec::new(),
            grid_width: 0,
            lookahead: APF_LOOKAHEAD_STEPS,
            attractive_strength: APF_ATTRACTIVE_STRENGTH,
            repulsive_radius: APF_REPULSIVE_RADIUS,
            repulsive_strength: APF_REPULSIVE_STRENGTH,
        }
    }

    fn add_repulsive(&mut self, center: IVec2, grid_w: i32, grid_h: i32) {
        let r = self.repulsive_radius;
        for dy in -r..=r {
            for dx in -r..=r {
                let p = center + IVec2::new(dx, dy);
                if p.x >= 0 && p.x < grid_w && p.y >= 0 && p.y < grid_h {
                    let dist = (dx.abs() + dy.abs()) as f64;
                    if dist > 0.0 && dist <= r as f64 {
                        let idx = (p.y * grid_w + p.x) as usize;
                        if idx < self.repulsive_field.len() {
                            self.repulsive_field[idx] +=
                                self.repulsive_strength * (1.0 - dist / (r as f64 + 1.0));
                        }
                    }
                }
            }
        }
    }
}

impl GuidanceLayer for ApfGuidance {
    fn name(&self) -> &'static str {
        "apf"
    }

    fn compute_guidance(
        &mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &DistanceMapCache,
    ) {
        let n = agents.len();
        let w = ctx.grid.width;
        let h = ctx.grid.height;
        let cells = (w * h) as usize;

        self.grid_width = w;

        // Build per-agent waypoints: trace optimal path forward for lookahead steps
        self.waypoints.clear();
        self.waypoints.resize(n, Vec::new());

        for (i, a) in agents.iter().enumerate() {
            let goal = a.goal.unwrap_or(a.pos);
            if a.pos == goal {
                continue;
            }

            let dm = match distance_cache.get_cached(goal) {
                Some(dm) => dm,
                None => continue,
            };

            let mut pos = a.pos;
            let mut path = Vec::with_capacity(self.lookahead);

            for _ in 0..self.lookahead {
                if pos == goal {
                    break;
                }

                let mut best = pos;
                let mut best_d = dm.get(pos);

                for dir in Direction::ALL {
                    let next = pos + dir.offset();
                    if ctx.grid.is_walkable(next) {
                        let d = dm.get(next);
                        if d < best_d {
                            best_d = d;
                            best = next;
                        }
                    }
                }

                if best == pos {
                    break;
                }
                pos = best;
                path.push(pos);
            }

            self.waypoints[i] = path;
        }

        // Build repulsive field from all agents' current positions
        if self.repulsive_field.len() != cells {
            self.repulsive_field = vec![0.0; cells];
        } else {
            self.repulsive_field.fill(0.0);
        }

        for a in agents {
            self.add_repulsive(a.pos, w, h);
        }
    }

    fn cell_bias(&self, pos: IVec2, agent_index: usize) -> f64 {
        let mut bias = 0.0;

        // Attractive: pull toward waypoints
        if agent_index < self.waypoints.len() {
            for (step, &wp) in self.waypoints[agent_index].iter().enumerate() {
                let dist = (pos - wp).as_vec2().length();
                if dist < 3.0 {
                    let weight = 1.0 / (step as f64 + 1.0);
                    bias += self.attractive_strength * weight * (1.0 - dist as f64 / 3.0);
                }
            }
        }

        // Repulsive: push away from congested areas
        let idx = (pos.y * self.grid_width + pos.x) as usize;
        if idx < self.repulsive_field.len() {
            bias += self.repulsive_field[idx];
        }

        bias
    }

    fn reset(&mut self) {
        self.waypoints.clear();
        self.repulsive_field.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
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
    fn apf_waypoints_computed() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut cache = DistanceMapCache::default();
        let mut apf = ApfGuidance::new(25, 1);

        // Pre-compute distance map for goal
        let pairs = [(IVec2::new(0, 0), IVec2::new(4, 4))];
        let _ = cache.get_or_compute(&grid, &pairs);

        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::new(0, 0),
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 4)),
        }];

        let ctx = SolverContext {
            grid: &grid,
            zones: &zones,
            tick: 0,
            num_agents: 1,
        };
        apf.compute_guidance(&ctx, &agents, &cache);
        assert!(
            !apf.waypoints[0].is_empty(),
            "should have computed waypoints"
        );
    }

    #[test]
    fn apf_repulsive_near_agent() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let cache = DistanceMapCache::default();
        let mut apf = ApfGuidance::new(25, 1);

        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::new(2, 2),
            goal: Some(IVec2::new(2, 2)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];

        let ctx = SolverContext {
            grid: &grid,
            zones: &zones,
            tick: 0,
            num_agents: 1,
        };
        apf.compute_guidance(&ctx, &agents, &cache);

        let idx = (3 * 5 + 2) as usize; // cell (2, 3)
        assert!(
            apf.repulsive_field[idx] > 0.0,
            "should have repulsive field near agent"
        );
    }

    #[test]
    fn apf_reset_clears_state() {
        let mut apf = ApfGuidance::new(25, 1);
        apf.waypoints.push(vec![IVec2::ZERO]);
        apf.reset();
        assert!(apf.waypoints.is_empty());
    }
}
