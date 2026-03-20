use bevy::prelude::*;
use std::collections::VecDeque;

use crate::core::action::{Action, Direction};
use crate::core::grid::GridMap;

// ---------------------------------------------------------------------------
// Distance functions
// ---------------------------------------------------------------------------

/// Manhattan distance (L1 norm) between two grid positions.
pub fn manhattan(a: IVec2, b: IVec2) -> u64 {
    let diff = a - b;
    (diff.x.unsigned_abs() + diff.y.unsigned_abs()) as u64
}

/// Chebyshev distance (L∞ norm, allows diagonal movement).
pub fn chebyshev(a: IVec2, b: IVec2) -> u64 {
    let diff = a - b;
    diff.x.unsigned_abs().max(diff.y.unsigned_abs()) as u64
}

// ---------------------------------------------------------------------------
// BFS distance map — flat Vec for cache-friendly O(1) lookups
// ---------------------------------------------------------------------------

/// Precomputed shortest-path distances from every reachable cell to a single
/// goal cell, computed via BFS on the grid (ignoring other agents).
///
/// Uses a flat `Vec<u64>` indexed by `(y * width + x)` instead of HashMap.
/// This gives ~10× speedup over HashMap for BFS flood-fill and lookups:
/// no hashing, no bucket management, cache-friendly sequential access.
pub struct DistanceMap {
    distances: Vec<u64>,
    width: i32,
}

impl DistanceMap {
    /// BFS flood-fill from `goal` on `grid`. O(width × height).
    pub fn compute(grid: &GridMap, goal: IVec2) -> Self {
        let w = grid.width;
        let h = grid.height;
        let size = (w * h) as usize;
        let mut distances = vec![u64::MAX; size];
        let mut queue = VecDeque::with_capacity(size);

        if goal.x >= 0 && goal.x < w && goal.y >= 0 && goal.y < h {
            let idx = (goal.y * w + goal.x) as usize;
            distances[idx] = 0;
            queue.push_back(goal);
        }

        while let Some(pos) = queue.pop_front() {
            let d = distances[(pos.y * w + pos.x) as usize];
            for dir in Direction::ALL {
                let next = pos + dir.offset();
                if grid.is_walkable(next) {
                    let next_idx = (next.y * w + next.x) as usize;
                    if distances[next_idx] == u64::MAX {
                        distances[next_idx] = d + 1;
                        queue.push_back(next);
                    }
                }
            }
        }

        Self { distances, width: w }
    }

    /// Exact distance from `pos` to the goal this map was built for.
    /// Returns `u64::MAX` if `pos` is unreachable or out of bounds.
    #[inline]
    pub fn get(&self, pos: IVec2) -> u64 {
        let idx = pos.y * self.width + pos.x;
        if idx < 0 || (idx as usize) >= self.distances.len() {
            return u64::MAX;
        }
        self.distances[idx as usize]
    }
}

/// Batch-compute distance maps for all agent goals.
/// Returns `Vec<DistanceMap>` aligned with the `agents` slice.
pub fn compute_distance_maps(grid: &GridMap, agents: &[(IVec2, IVec2)]) -> Vec<DistanceMap> {
    agents
        .iter()
        .map(|&(_, goal)| DistanceMap::compute(grid, goal))
        .collect()
}

// ---------------------------------------------------------------------------
// Distance map cache — reuse BFS maps across lifelong replans
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use bevy::prelude::Resource;

/// Caches BFS distance maps keyed by goal position.
/// In lifelong mode most agents keep the same goal across replans — only agents
/// who just completed tasks get new goals. This avoids rerunning N BFS flood-fills
/// when only 1-5 goals changed.
///
/// Obstacle change strategy: when an obstacle is added (agent dies), only evict
/// cached maps whose BFS path passes through the new obstacle cell. Maps that
/// never use that cell remain valid. This avoids the catastrophic N×BFS full
/// recompute that the previous "clear all on obstacle count change" caused.
#[derive(Resource, Default)]
pub struct DistanceMapCache {
    cache: HashMap<IVec2, DistanceMap>,
    /// Grid dimensions when cache was built. Invalidate on grid change.
    grid_w: i32,
    grid_h: i32,
    /// Obstacle count when cache was built — detect when grid changed.
    grid_obstacle_count: usize,
}

impl DistanceMapCache {
    /// Get or compute distance maps for the given agents, returning them aligned
    /// with the `agents` slice. Reuses cached maps where goals haven't changed.
    pub fn get_or_compute(
        &mut self,
        grid: &GridMap,
        agents: &[(IVec2, IVec2)],
    ) -> Vec<&DistanceMap> {
        let obstacle_count = grid.obstacle_count();

        // Full invalidation only on grid dimension change
        if grid.width != self.grid_w || grid.height != self.grid_h {
            self.cache.clear();
            self.grid_w = grid.width;
            self.grid_h = grid.height;
            self.grid_obstacle_count = obstacle_count;
        } else if obstacle_count != self.grid_obstacle_count {
            // Obstacle count changed (fault killed agent / placed obstacle).
            // Selective invalidation: evict only maps that routed through a
            // now-blocked cell. A cached distance map is invalid if any cell
            // that is now an obstacle had a finite (reachable) distance in it.
            // We iterate obstacles and check each cached map's value at that
            // cell — if it was reachable (not u64::MAX), the map is stale.
            //
            // This is O(obstacles × cached_goals) per obstacle-change tick,
            // but obstacle changes are rare events (fault ticks), not every-tick.
            // The alternative (full clear) is O(active_goals × grid_size) BFS
            // floods which is far more expensive.
            self.cache.retain(|_goal, dm| {
                // Keep this map only if no current obstacle was reachable in it.
                // We only need to check obstacles that were added since last time.
                // Since we can't easily diff obstacles, check all obstacles —
                // but this only runs on fault ticks, not every tick.
                for &obs in grid.obstacles() {
                    if dm.get(obs) != u64::MAX {
                        return false; // This map routed through a now-blocked cell
                    }
                }
                true
            });
            self.grid_obstacle_count = obstacle_count;
        }

        // Ensure all goals are in the cache
        for &(_, goal) in agents {
            self.cache
                .entry(goal)
                .or_insert_with(|| DistanceMap::compute(grid, goal));
        }

        // Return references aligned with agents
        agents
            .iter()
            .map(|&(_, goal)| self.cache.get(&goal).unwrap())
            .collect()
    }

    /// Evict entries for goals no longer in use (call periodically to bound memory).
    pub fn retain_goals(&mut self, active_goals: &[IVec2]) {
        let active: std::collections::HashSet<IVec2> = active_goals.iter().copied().collect();
        self.cache.retain(|goal, _| active.contains(goal));
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.grid_w = 0;
        self.grid_h = 0;
        self.grid_obstacle_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;

    fn open5() -> GridMap {
        GridMap::new(5, 5)
    }

    // ── manhattan ──────────────────────────────────────────────────────────

    #[test]
    fn manhattan_same_cell_is_zero() {
        let p = IVec2::new(3, 3);
        assert_eq!(manhattan(p, p), 0);
    }

    #[test]
    fn manhattan_axis_aligned() {
        assert_eq!(manhattan(IVec2::ZERO, IVec2::new(4, 0)), 4);
        assert_eq!(manhattan(IVec2::ZERO, IVec2::new(0, 4)), 4);
    }

    #[test]
    fn manhattan_diagonal_is_sum() {
        assert_eq!(manhattan(IVec2::ZERO, IVec2::new(3, 4)), 7);
    }

    #[test]
    fn manhattan_is_symmetric() {
        let a = IVec2::new(1, 2);
        let b = IVec2::new(4, 5);
        assert_eq!(manhattan(a, b), manhattan(b, a));
    }

    // ── chebyshev ─────────────────────────────────────────────────────────

    #[test]
    fn chebyshev_same_cell_is_zero() {
        let p = IVec2::new(2, 2);
        assert_eq!(chebyshev(p, p), 0);
    }

    #[test]
    fn chebyshev_diagonal_is_max_component() {
        assert_eq!(chebyshev(IVec2::ZERO, IVec2::new(3, 4)), 4);
        assert_eq!(chebyshev(IVec2::ZERO, IVec2::new(4, 3)), 4);
    }

    #[test]
    fn chebyshev_axis_aligned_matches_manhattan() {
        assert_eq!(chebyshev(IVec2::ZERO, IVec2::new(5, 0)), 5);
    }

    // ── DistanceMap ───────────────────────────────────────────────────────

    #[test]
    fn distance_map_goal_is_zero() {
        let grid = open5();
        let goal = IVec2::new(4, 4);
        let dm = DistanceMap::compute(&grid, goal);
        assert_eq!(dm.get(goal), 0);
    }

    #[test]
    fn distance_map_open_grid_matches_manhattan() {
        let grid = open5();
        let goal = IVec2::new(4, 4);
        let dm = DistanceMap::compute(&grid, goal);
        // In an open grid BFS = Manhattan distance
        assert_eq!(dm.get(IVec2::ZERO), 8);
        assert_eq!(dm.get(IVec2::new(4, 0)), 4);
        assert_eq!(dm.get(IVec2::new(0, 4)), 4);
    }

    #[test]
    fn distance_map_obstacle_cell_is_unreachable() {
        // An obstacle that is NOT the goal should never be reachable by BFS.
        let mut grid = GridMap::new(3, 3);
        grid.set_obstacle(IVec2::new(1, 1));
        let dm = DistanceMap::compute(&grid, IVec2::new(2, 2)); // goal is open
        assert_eq!(dm.get(IVec2::new(1, 1)), u64::MAX);
    }

    #[test]
    fn distance_map_isolated_region_is_unreachable() {
        let mut grid = GridMap::new(5, 1);
        // Block the corridor: 0 | wall | 2 3 4 (goal=4 unreachable from 0)
        grid.set_obstacle(IVec2::new(1, 0));
        let dm = DistanceMap::compute(&grid, IVec2::new(4, 0));
        assert_eq!(dm.get(IVec2::ZERO), u64::MAX);
    }

    #[test]
    fn distance_map_out_of_bounds_is_max() {
        let grid = open5();
        let dm = DistanceMap::compute(&grid, IVec2::new(2, 2));
        assert_eq!(dm.get(IVec2::new(-1, 0)), u64::MAX);
        assert_eq!(dm.get(IVec2::new(10, 10)), u64::MAX);
    }

    #[test]
    fn compute_distance_maps_returns_one_per_agent() {
        let grid = open5();
        let agents = vec![
            (IVec2::ZERO, IVec2::new(4, 4)),
            (IVec2::new(4, 0), IVec2::new(0, 4)),
        ];
        let maps = compute_distance_maps(&grid, &agents);
        assert_eq!(maps.len(), 2);
        assert_eq!(maps[0].get(IVec2::new(4, 4)), 0);
        assert_eq!(maps[1].get(IVec2::new(0, 4)), 0);
    }

    // ── delta_to_action ───────────────────────────────────────────────────

    #[test]
    fn delta_to_action_same_pos_is_wait() {
        let p = IVec2::new(2, 2);
        assert_eq!(delta_to_action(p, p), Action::Wait);
    }

    #[test]
    fn delta_to_action_cardinal_directions() {
        let origin = IVec2::new(2, 2);
        assert_eq!(delta_to_action(origin, IVec2::new(2, 3)), Action::Move(Direction::North));
        assert_eq!(delta_to_action(origin, IVec2::new(2, 1)), Action::Move(Direction::South));
        assert_eq!(delta_to_action(origin, IVec2::new(3, 2)), Action::Move(Direction::East));
        assert_eq!(delta_to_action(origin, IVec2::new(1, 2)), Action::Move(Direction::West));
    }
}

// ---------------------------------------------------------------------------
// Action helpers
// ---------------------------------------------------------------------------

/// Convert a position delta (from → to) into an `Action`.
/// If `from == to`, returns `Action::Wait`.
/// Panics in debug if the delta is not a cardinal move or wait.
pub fn delta_to_action(from: IVec2, to: IVec2) -> Action {
    let diff = to - from;
    match (diff.x, diff.y) {
        (0, 0) => Action::Wait,
        (0, 1) => Action::Move(Direction::North),
        (0, -1) => Action::Move(Direction::South),
        (1, 0) => Action::Move(Direction::East),
        (-1, 0) => Action::Move(Direction::West),
        _ => {
            debug_assert!(false, "delta_to_action: invalid delta {diff}");
            Action::Wait
        }
    }
}
