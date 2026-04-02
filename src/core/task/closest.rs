use bevy::prelude::IVec2;
use rand_chacha::ChaCha8Rng;
use std::collections::HashSet;

use super::super::topology::ZoneMap;
use super::random::random_cell_from;
use super::TaskScheduler;

// ---------------------------------------------------------------------------
// ClosestFirstScheduler
// ---------------------------------------------------------------------------

pub struct ClosestFirstScheduler;

impl ClosestFirstScheduler {
    fn nearest_from_cells(cells: &[IVec2], pos: IVec2, occupied: &HashSet<IVec2>) -> Option<IVec2> {
        if cells.is_empty() {
            return None;
        }
        // Prefer closest unoccupied cell
        let best = cells
            .iter()
            .copied()
            .filter(|&c| c != pos && !occupied.contains(&c))
            .min_by_key(|c| (c.x - pos.x).abs() + (c.y - pos.y).abs());
        if best.is_some() {
            return best;
        }
        // All cells claimed — caller should keep agent waiting
        None
    }
}

impl TaskScheduler for ClosestFirstScheduler {
    fn name(&self) -> &str {
        "closest"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        _rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::nearest_from_cells(&zones.pickup_cells, pos, occupied)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        _pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        random_cell_from(&zones.delivery_cells, occupied, rng)
    }

    fn assign_pickups_batch(
        &self,
        free_agents: &[(usize, IVec2)],
        zones: &ZoneMap,
        used_goals: &mut HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Vec<(usize, IVec2)> {
        if free_agents.is_empty() {
            return Vec::new();
        }

        // Phase 1 — task creation: randomly generate one candidate pickup per free agent.
        // Creation is uniform-random and position-independent — no agent-position bias.
        let mut pool_used = used_goals.clone();
        let mut task_pool: Vec<IVec2> = Vec::with_capacity(free_agents.len());
        for _ in 0..free_agents.len() {
            if let Some(cell) = random_cell_from(&zones.pickup_cells, &pool_used, rng) {
                pool_used.insert(cell);
                task_pool.push(cell);
            } else {
                break; // No more available pickup cells
            }
        }
        if task_pool.is_empty() {
            return Vec::new();
        }

        // Phase 2 — task assignment: each agent picks the nearest task in the
        // random pool. This avoids convergence on a fixed hotspot because the
        // pool is random; agents still prefer shorter trips.
        let mut available = task_pool;
        let mut assignments = Vec::with_capacity(available.len());
        for &(idx, pos) in free_agents {
            if available.is_empty() {
                break;
            }
            let best = available
                .iter()
                .enumerate()
                .min_by_key(|&(_, c)| (c.x - pos.x).abs() + (c.y - pos.y).abs());
            if let Some((ti, &pickup)) = best {
                available.swap_remove(ti);
                used_goals.insert(pickup);
                assignments.push((idx, pickup));
            }
        }
        assignments
    }
}
