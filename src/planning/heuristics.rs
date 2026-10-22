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
    agents.iter().map(|&(_, goal)| DistanceMap::compute(grid, goal)).collect()
}

// ---------------------------------------------------------------------------
// Distance map cache — reuse BFS maps across lifelong replans
// ---------------------------------------------------------------------------

use bevy::prelude::Resource;
use std::collections::HashMap;

/// Caches BFS distance maps keyed by goal position.
/// In lifelong mode most agents keep the same goal across replans — only agents
/// who just completed tasks get new goals. This avoids rerunning N BFS flood-fills
/// when only 1-5 goals changed.
///
/// **Pooled BFS scratch** (RHCR-PBS perf sprint 2026-04-09): the BFS `VecDeque`
/// used to flood-fill each goal is pooled on the cache itself so it survives
/// across the N-per-window cache-miss burst. Eliminates ~N × 15 KB of queue
/// allocation per cold first tick.
///
/// **Obstacle diff eviction** (RHCR-PBS perf sprint 2026-04-09): only walks
/// cached maps against *newly-added* obstacles (by diffing the current
/// obstacle set against a stored snapshot), not all current obstacles. The
/// previous code walked every obstacle for every map and evicted on any
/// finite-distance hit, which was effectively "evict-all on any fault" —
/// because cached maps were computed before the obstacle was added, every
/// walkable obstacle cell was reachable. This made the post-fault FPS dip
/// worse by forcing a full re-BFS.
#[derive(Resource, Default)]
pub struct DistanceMapCache {
    cache: HashMap<IVec2, DistanceMap>,
    /// Grid dimensions when cache was built. Invalidate on grid change.
    grid_w: i32,
    grid_h: i32,
    /// Obstacle snapshot at last `get_or_compute` call. Diffed against the
    /// current grid to compute `added_obstacles` on entry, so we only evict
    /// maps that route through a cell that just became blocked.
    last_obstacles: Vec<IVec2>,
    /// Pooled BFS queue, reused across `compute_with_pooled_queue` calls.
    /// Cleared at the start of each BFS, grown on demand.
    bfs_queue: VecDeque<IVec2>,
    /// Scratch buffer reused by the obstacle-diff logic across calls —
    /// avoids a small per-call Vec allocation on fault ticks.
    added_obstacles_scratch: Vec<IVec2>,
}

impl DistanceMapCache {
    /// Get or compute distance maps for the given agents, returning them aligned
    /// with the `agents` slice. Reuses cached maps where goals haven't changed.
    pub fn get_or_compute(
        &mut self,
        grid: &GridMap,
        agents: &[(IVec2, IVec2)],
    ) -> Vec<&DistanceMap> {
        // Full invalidation only on grid dimension change
        if grid.width != self.grid_w || grid.height != self.grid_h {
            self.cache.clear();
            self.grid_w = grid.width;
            self.grid_h = grid.height;
            self.last_obstacles.clear();
            self.last_obstacles.extend(grid.obstacles().iter().copied());
        } else if grid.obstacle_count() != self.last_obstacles.len() {
            // Obstacle set changed — find newly-added cells. A cached map is
            // invalid iff its BFS routed *through* one of those newly-blocked
            // cells (i.e., the cell had finite distance in the cached map).
            // Maps that never touched the new obstacle remain valid.
            //
            // For a typical fault tick `added` has length 1, so the
            // containment check inside the retain loop is O(cached_maps) —
            // far cheaper than the previous "walk every current obstacle for
            // every map" approach (which evicted everything unconditionally
            // because the cached map was computed before the obstacle was
            // added).
            self.added_obstacles_scratch.clear();
            for &obs in grid.obstacles() {
                if !self.last_obstacles.contains(&obs) {
                    self.added_obstacles_scratch.push(obs);
                }
            }

            if !self.added_obstacles_scratch.is_empty() {
                let added = &self.added_obstacles_scratch;
                self.cache.retain(|_goal, dm| {
                    // Keep the map only if NONE of the newly-added obstacle
                    // cells were reachable in it.
                    added.iter().all(|&o| dm.get(o) == u64::MAX)
                });
            }

            // Snapshot the new obstacle set for the next diff.
            self.last_obstacles.clear();
            self.last_obstacles.extend(grid.obstacles().iter().copied());
        }

        // Ensure all goals are in the cache. Can't use `entry().or_insert_with`
        // because the closure would need to borrow `self` mutably while the
        // entry already holds a mutable borrow on `self.cache` — conflicts
        // with the pooled `bfs_queue` access inside `compute_with_pooled_queue`.
        // Two-phase: check-then-compute.
        for &(_, goal) in agents {
            if !self.cache.contains_key(&goal) {
                let dm = self.compute_with_pooled_queue(grid, goal);
                self.cache.insert(goal, dm);
            }
        }

        // Return references aligned with agents
        agents.iter().map(|&(_, goal)| self.cache.get(&goal).unwrap()).collect()
    }

    /// BFS flood-fill from `goal` using the cache's pooled `VecDeque`, so the
    /// queue's backing allocation is reused across successive cache-miss
    /// computes. The returned `DistanceMap` owns a fresh `Vec<u64>` for the
    /// distance array (which lives as long as the cache entry), so only the
    /// queue is pooled — not the output.
    fn compute_with_pooled_queue(&mut self, grid: &GridMap, goal: IVec2) -> DistanceMap {
        let w = grid.width;
        let h = grid.height;
        let size = (w * h) as usize;
        let mut distances = vec![u64::MAX; size];
        self.bfs_queue.clear();

        if goal.x >= 0 && goal.x < w && goal.y >= 0 && goal.y < h {
            let idx = (goal.y * w + goal.x) as usize;
            distances[idx] = 0;
            self.bfs_queue.push_back(goal);
        }

        while let Some(pos) = self.bfs_queue.pop_front() {
            let d = distances[(pos.y * w + pos.x) as usize];
            for dir in Direction::ALL {
                let next = pos + dir.offset();
                if grid.is_walkable(next) {
                    let next_idx = (next.y * w + next.x) as usize;
                    if distances[next_idx] == u64::MAX {
                        distances[next_idx] = d + 1;
                        self.bfs_queue.push_back(next);
                    }
                }
            }
        }

        DistanceMap { distances, width: w }
    }

    /// Pre-compute distance maps for a set of goal cells, populating the cache
    /// synchronously on the calling thread. Used by `SimulationRunner::new` to
    /// move the N-BFS cold-start cost off the first rendered tick: the user
    /// sees a brief "Start" → "first tick" delay (expected loading behavior)
    /// instead of a frozen frame-to-frame animation.
    ///
    /// Goals already in the cache are skipped. Invalidation rules in
    /// `get_or_compute` still apply on subsequent calls (obstacle diff /
    /// dimension change), so this is a pure cold-cache warm-up.
    pub fn warm_up_goals(&mut self, grid: &GridMap, goals: &[IVec2]) {
        // Prime the dimension/obstacle state so the next `get_or_compute` call
        // doesn't undo our work by clearing the cache on a dimension mismatch.
        if grid.width != self.grid_w || grid.height != self.grid_h {
            self.cache.clear();
            self.grid_w = grid.width;
            self.grid_h = grid.height;
            self.last_obstacles.clear();
            self.last_obstacles.extend(grid.obstacles().iter().copied());
        }

        for &goal in goals {
            if !self.cache.contains_key(&goal) {
                let dm = self.compute_with_pooled_queue(grid, goal);
                self.cache.insert(goal, dm);
            }
        }
    }

    /// Evict entries for goals no longer in use (call periodically to bound memory).
    pub fn retain_goals(&mut self, active_goals: &[IVec2]) {
        let active: std::collections::HashSet<IVec2> = active_goals.iter().copied().collect();
        self.cache.retain(|goal, _| active.contains(goal));
    }

    /// Get a cached distance map without computing. Returns None if not cached.
    pub fn get_cached(&self, goal: IVec2) -> Option<&DistanceMap> {
        self.cache.get(&goal)
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.grid_w = 0;
        self.grid_h = 0;
        self.last_obstacles.clear();
        // Leave bfs_queue capacity intact — it's a reusable pool.
    }

    /// Number of cached distance maps. Useful for tests verifying that the
    /// cache holds the expected unique-goal count, and for runtime memory
    /// monitoring (each entry is `grid_area × 8 bytes`).
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Whether the cache currently holds any distance maps.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
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
        let agents = vec![(IVec2::ZERO, IVec2::new(4, 4)), (IVec2::new(4, 0), IVec2::new(0, 4))];
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
