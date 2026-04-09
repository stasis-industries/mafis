use bevy::prelude::IVec2;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use smallvec::SmallVec;
use std::collections::HashSet;

use super::super::topology::ZoneMap;
use super::{TaskLeg, TaskScheduler};
use crate::constants::{PBS_GOAL_SEQUENCE_MAX_LEN, PEEK_CHAIN_MAX_RETRIES};

// ---------------------------------------------------------------------------
// Task creation helper
// ---------------------------------------------------------------------------

/// Select one cell uniformly at random from `cells` that is not in `occupied`.
///
/// Used for **task creation** (generating a new task regardless of which agent
/// will be assigned to it). Unlike per-agent selection, there is no agent-position
/// bias — any available cell is equally valid.
pub fn random_cell_from(
    cells: &[IVec2],
    occupied: &HashSet<IVec2>,
    rng: &mut ChaCha8Rng,
) -> Option<IVec2> {
    if cells.is_empty() {
        return None;
    }
    // Rejection sampling — fast in the common case (most cells free)
    for _ in 0..200 {
        let idx = rng.random_range(0..cells.len());
        let cell = cells[idx];
        if !occupied.contains(&cell) {
            return Some(cell);
        }
    }
    // Fallback: collect all available cells and pick uniformly
    let valid: Vec<IVec2> = cells.iter().copied().filter(|c| !occupied.contains(c)).collect();
    if valid.is_empty() { None } else { Some(valid[rng.random_range(0..valid.len())]) }
}

// ---------------------------------------------------------------------------
// RandomScheduler
// ---------------------------------------------------------------------------

pub struct RandomScheduler;

impl RandomScheduler {
    fn random_from_cells(
        cells: &[IVec2],
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        if cells.is_empty() {
            return None;
        }
        // Try random sampling first
        for _ in 0..200 {
            let idx = rng.random_range(0..cells.len());
            let cell = cells[idx];
            if cell != pos && !occupied.contains(&cell) {
                return Some(cell);
            }
        }
        // Fallback: linear scan
        let valid: Vec<IVec2> =
            cells.iter().copied().filter(|&c| c != pos && !occupied.contains(&c)).collect();
        if valid.is_empty() {
            // All cells claimed — caller should keep agent waiting
            None
        } else {
            Some(valid[rng.random_range(0..valid.len())])
        }
    }
}

impl TaskScheduler for RandomScheduler {
    fn name(&self) -> &str {
        "random"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::random_from_cells(&zones.pickup_cells, pos, occupied, rng)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::random_from_cells(&zones.delivery_cells, pos, occupied, rng)
    }

    /// Mirror `KivaSystem.cpp:143-196`'s alternating-endpoint chain.
    ///
    /// Determines the next leg type from `current_leg`, then samples uniformly
    /// from the matching zone (pickup or delivery), rejecting any sample equal
    /// to the previous chain head (`current_goal` for the first element, the
    /// last accepted chain element thereafter). Terminates on distance budget,
    /// length cap, or `PEEK_CHAIN_MAX_RETRIES` consecutive rejections.
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

        // Empty zones short-circuit: nothing to chain.
        if zones.pickup_cells.is_empty() && zones.delivery_cells.is_empty() {
            return chain;
        }

        // Determine the type of the *next* element to add after `current_goal`.
        // If the agent is currently heading to a pickup (or has no task), the
        // next chain element is a delivery. Otherwise the next element is a pickup.
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

            let idx = rng.random_range(0..cells.len());
            let candidate = cells[idx];

            // Reject candidates equal to current_goal OR equal to the previously
            // chosen chain head (mirror `KivaSystem.cpp:83-85`).
            if candidate == current_goal || candidate == chain_head {
                consecutive_rejections += 1;
                continue;
            }

            // Compute the additional Manhattan distance for this hop.
            let hop_distance =
                ((candidate.x - chain_head.x).abs() + (candidate.y - chain_head.y).abs()) as u64;
            let new_cumulative = cumulative_distance.saturating_add(hop_distance);
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
        // Pickups intentionally separated from deliveries so chain hops have
        // non-trivial Manhattan distances.
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
    fn random_peek_chain_terminates_on_distance_budget() {
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
        // Budget = 5 cells of cumulative Manhattan distance
        let budget = 5u64;

        // Try several seeds — total distance must always be <= 5
        for seed in 0..32u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let chain = scheduler.peek_task_chain(
                &zones,
                IVec2::new(5, 4),
                IVec2::new(5, 4), // current_goal
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
    fn random_peek_chain_alternates_pickup_delivery_by_leg() {
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
        let mut rng = ChaCha8Rng::seed_from_u64(7);

        // Starting leg = TravelEmpty (heading to pickup) → next chain element = delivery
        let chain = scheduler.peek_task_chain(
            &zones,
            IVec2::new(5, 6),
            IVec2::new(5, 6),
            TaskLeg::TravelEmpty(IVec2::new(5, 6)),
            10_000, // huge budget
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
    fn random_peek_chain_alternates_for_loaded_leg() {
        // Inverse alternation: TravelLoaded (heading to delivery) → next = pickup
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
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
    fn random_peek_chain_excludes_current_goal_from_first_element() {
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
        // Set leg so the first chain element is a pickup, then set current_goal
        // to a specific pickup cell — chain[0] must never equal that goal.
        let pinned_pickup = IVec2::new(7, 4);
        for seed in 0..50u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            // TravelLoaded → next chain element is pickup
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
    fn random_peek_chain_deterministic_under_same_rng() {
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
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
    fn random_peek_chain_caps_at_max_len() {
        let zones = make_chain_zones();
        let scheduler = RandomScheduler;
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
