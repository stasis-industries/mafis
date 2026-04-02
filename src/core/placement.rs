//! Shared agent placement functions used by both the live ECS (`controls.rs`)
//! and the headless baseline engine (`baseline.rs`).
//!
//! Having a single implementation guarantees identical RNG consumption,
//! which is critical for headless-vs-live parity.

use bevy::math::IVec2;
use rand::Rng;
use std::collections::HashSet;

use super::grid::GridMap;

/// Pick a random cell from `pool` that is not in `exclude`.
/// Falls back to collecting all valid cells and picking one.
pub fn find_from_pool(
    pool: &[IVec2],
    rng: &mut impl Rng,
    exclude: &HashSet<IVec2>,
) -> Option<IVec2> {
    for _ in 0..200 {
        let idx = rng.random_range(0..pool.len());
        let pos = pool[idx];
        if !exclude.contains(&pos) {
            return Some(pos);
        }
    }
    let valid: Vec<IVec2> = pool.iter().copied().filter(|p| !exclude.contains(p)).collect();
    if valid.is_empty() { None } else { Some(valid[rng.random_range(0..valid.len())]) }
}

/// Pick a random walkable cell on `grid` that is not in `exclude`.
/// Falls back to collecting all valid cells and picking one.
pub fn find_random_walkable(grid: &GridMap, rng: &mut impl Rng, exclude: &HashSet<IVec2>) -> IVec2 {
    for _ in 0..200 {
        let pos = IVec2::new(rng.random_range(0..grid.width), rng.random_range(0..grid.height));
        if grid.is_walkable(pos) && !exclude.contains(&pos) {
            return pos;
        }
    }
    // Fallback: collect all valid cells and pick one
    let mut valid: Vec<IVec2> = Vec::new();
    for x in 0..grid.width {
        for y in 0..grid.height {
            let pos = IVec2::new(x, y);
            if grid.is_walkable(pos) && !exclude.contains(&pos) {
                valid.push(pos);
            }
        }
    }
    if valid.is_empty() { IVec2::ZERO } else { valid[rng.random_range(0..valid.len())] }
}
