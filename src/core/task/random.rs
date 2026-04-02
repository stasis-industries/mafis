use bevy::prelude::IVec2;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use std::collections::HashSet;

use super::super::topology::ZoneMap;
use super::TaskScheduler;

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
}
