use bevy::prelude::IVec2;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use smallvec::SmallVec;
use std::collections::HashSet;

use super::super::topology::ZoneMap;
use super::random::random_cell_from;
use super::{TaskLeg, TaskScheduler};
use crate::constants::{PBS_GOAL_SEQUENCE_MAX_LEN, PEEK_CHAIN_MAX_RETRIES};

/// Random pool size used by `peek_task_chain` for the locality-aware sampler.
/// Mirrors the small random-then-nearest pattern used in `assign_pickups_batch`.
const PEEK_CHAIN_POOL_SIZE: usize = 8;

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

    /// Locality-aware peek chain: alternates pickup/delivery (like `RandomScheduler`)
    /// but each new chain element is the closest cell in a small random pool to
    /// the current chain head. Mirrors `assign_pickups_batch`'s random-pool-then-closest
    /// pattern, scaled down for single-agent peek (no multi-agent coordination needed).
    fn peek_task_chain(
        &self,
        zones: &ZoneMap,
        _current_pos: IVec2,
        current_goal: IVec2,
        current_leg: TaskLeg,
        max_cumulative_distance: u64,
        rng: &mut ChaCha8Rng,
    ) -> SmallVec<[IVec2; 8]> {
        let mut chain: SmallVec<[IVec2; 8]> = SmallVec::new();

        if zones.pickup_cells.is_empty() && zones.delivery_cells.is_empty() {
            return chain;
        }

        // Same alternation rule as RandomScheduler.
        let mut next_is_pickup = matches!(
            current_leg,
            TaskLeg::TravelLoaded { .. }
                | TaskLeg::TravelToQueue { .. }
                | TaskLeg::Queuing { .. }
                | TaskLeg::Unloading { .. }
        );

        let mut chain_head = current_goal;
        let mut cumulative_distance: u64 = 0;
        let mut consecutive_rejections: usize = 0;

        loop {
            if chain.len() >= PBS_GOAL_SEQUENCE_MAX_LEN {
                break;
            }
            if consecutive_rejections >= PEEK_CHAIN_MAX_RETRIES {
                break;
            }

            let cells = if next_is_pickup { &zones.pickup_cells } else { &zones.delivery_cells };
            if cells.is_empty() {
                break;
            }

            // Sample a small random pool and pick the closest cell to chain_head.
            // Skip cells equal to current_goal or the previous chain element.
            let pool_size = PEEK_CHAIN_POOL_SIZE.min(cells.len());
            let mut best: Option<(IVec2, i64)> = None;
            for _ in 0..pool_size {
                let idx = rng.random_range(0..cells.len());
                let candidate = cells[idx];
                if candidate == current_goal || candidate == chain_head {
                    continue;
                }
                let dist = ((candidate.x - chain_head.x).abs() + (candidate.y - chain_head.y).abs())
                    as i64;
                match best {
                    None => best = Some((candidate, dist)),
                    Some((_, best_dist)) if dist < best_dist => best = Some((candidate, dist)),
                    _ => {}
                }
            }

            let Some((candidate, dist)) = best else {
                consecutive_rejections += 1;
                continue;
            };

            let new_cumulative = cumulative_distance.saturating_add(dist as u64);
            if new_cumulative > max_cumulative_distance {
                break;
            }

            chain.push(candidate);
            cumulative_distance = new_cumulative;
            chain_head = candidate;
            next_is_pickup = !next_is_pickup;
            consecutive_rejections = 0;
        }

        chain
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::topology::ZoneType;
    use rand::SeedableRng;
    use std::collections::HashMap;

    fn make_chain_zones() -> ZoneMap {
        // 12x12 zone map: y=0..2 = delivery, y=4 and y=6 = pickup, rest = corridor.
        let mut zone_type = HashMap::new();
        let mut pickup_cells = Vec::new();
        let mut delivery_cells = Vec::new();
        for x in 0..12 {
            for y in 0..12 {
                let pos = IVec2::new(x, y);
                if y < 2 {
                    zone_type.insert(pos, ZoneType::Delivery);
                    delivery_cells.push(pos);
                } else if y == 4 || y == 6 {
                    zone_type.insert(pos, ZoneType::Pickup);
                    pickup_cells.push(pos);
                } else {
                    zone_type.insert(pos, ZoneType::Corridor);
                }
            }
        }
        ZoneMap {
            pickup_cells,
            delivery_cells,
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type,
            queue_lines: Vec::new(),
        }
    }

    fn cumulative_distance(start: IVec2, chain: &[IVec2]) -> u64 {
        let mut prev = start;
        let mut total: u64 = 0;
        for &c in chain {
            total += ((c.x - prev.x).abs() + (c.y - prev.y).abs()) as u64;
            prev = c;
        }
        total
    }

    #[test]
    fn closest_peek_chain_terminates_on_distance_budget() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let budget = 5u64;
        for seed in 0..32u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let chain = scheduler.peek_task_chain(
                &zones,
                IVec2::new(5, 4),
                IVec2::new(5, 4),
                TaskLeg::TravelEmpty(IVec2::new(5, 4)),
                budget,
                &mut rng,
            );
            let total = cumulative_distance(IVec2::new(5, 4), &chain);
            assert!(
                total <= budget,
                "seed={seed}: cumulative distance {total} exceeded budget {budget}, chain={chain:?}"
            );
        }
    }

    #[test]
    fn closest_peek_chain_alternates_pickup_delivery_by_leg() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = ChaCha8Rng::seed_from_u64(7);

        // TravelEmpty (heading to pickup) → next chain element = delivery
        let chain = scheduler.peek_task_chain(
            &zones,
            IVec2::new(5, 6),
            IVec2::new(5, 6),
            TaskLeg::TravelEmpty(IVec2::new(5, 6)),
            10_000,
            &mut rng,
        );
        assert!(chain.len() >= 2, "chain too short to verify alternation: {chain:?}");
        for (i, cell) in chain.iter().enumerate() {
            if i % 2 == 0 {
                assert!(
                    zones.delivery_cells.contains(cell),
                    "chain[{i}] = {cell:?} should be delivery"
                );
            } else {
                assert!(
                    zones.pickup_cells.contains(cell),
                    "chain[{i}] = {cell:?} should be pickup"
                );
            }
        }
    }

    #[test]
    fn closest_peek_chain_alternates_for_loaded_leg() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let chain = scheduler.peek_task_chain(
            &zones,
            IVec2::new(5, 1),
            IVec2::new(5, 1),
            TaskLeg::TravelLoaded { from: IVec2::new(5, 6), to: IVec2::new(5, 1) },
            10_000,
            &mut rng,
        );
        assert!(chain.len() >= 2, "chain too short to verify alternation: {chain:?}");
        for (i, cell) in chain.iter().enumerate() {
            if i % 2 == 0 {
                assert!(
                    zones.pickup_cells.contains(cell),
                    "chain[{i}] = {cell:?} should be pickup"
                );
            } else {
                assert!(
                    zones.delivery_cells.contains(cell),
                    "chain[{i}] = {cell:?} should be delivery"
                );
            }
        }
    }

    #[test]
    fn closest_peek_chain_excludes_current_goal_from_first_element() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let pinned_pickup = IVec2::new(7, 4);
        for seed in 0..50u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let chain = scheduler.peek_task_chain(
                &zones,
                IVec2::new(7, 1),
                pinned_pickup,
                TaskLeg::TravelLoaded { from: IVec2::new(0, 0), to: pinned_pickup },
                10_000,
                &mut rng,
            );
            if let Some(first) = chain.first() {
                assert_ne!(*first, pinned_pickup, "seed={seed}: chain[0] equal to current_goal");
                assert!(
                    zones.pickup_cells.contains(first),
                    "seed={seed}: chain[0] = {first:?} should be pickup"
                );
            }
        }
    }

    #[test]
    fn closest_peek_chain_deterministic_under_same_rng() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng_a = ChaCha8Rng::seed_from_u64(42);
        let mut rng_b = ChaCha8Rng::seed_from_u64(42);
        let chain_a = scheduler.peek_task_chain(
            &zones,
            IVec2::new(3, 4),
            IVec2::new(3, 4),
            TaskLeg::TravelEmpty(IVec2::new(3, 4)),
            50,
            &mut rng_a,
        );
        let chain_b = scheduler.peek_task_chain(
            &zones,
            IVec2::new(3, 4),
            IVec2::new(3, 4),
            TaskLeg::TravelEmpty(IVec2::new(3, 4)),
            50,
            &mut rng_b,
        );
        assert_eq!(chain_a.as_slice(), chain_b.as_slice());
        assert!(!chain_a.is_empty(), "chain should be non-empty for budget=50");
    }

    #[test]
    fn closest_peek_chain_caps_at_max_len() {
        let zones = make_chain_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = ChaCha8Rng::seed_from_u64(99);
        let chain = scheduler.peek_task_chain(
            &zones,
            IVec2::new(5, 4),
            IVec2::new(5, 4),
            TaskLeg::TravelEmpty(IVec2::new(5, 4)),
            u64::MAX,
            &mut rng,
        );
        assert!(
            chain.len() <= PBS_GOAL_SEQUENCE_MAX_LEN,
            "chain length {} exceeds cap {}",
            chain.len(),
            PBS_GOAL_SEQUENCE_MAX_LEN
        );
        assert_eq!(PBS_GOAL_SEQUENCE_MAX_LEN, 8);
    }
}
