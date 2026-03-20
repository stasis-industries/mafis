use bevy::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::core::action::{Action, Direction};
use crate::core::grid::GridMap;

use super::heuristics::{DistanceMap, manhattan};
use super::traits::SolverError;

// ---------------------------------------------------------------------------
// Legacy constraint types (kept for PBS planner compatibility)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VertexConstraint {
    pub agent: usize,
    pub pos: IVec2,
    pub time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EdgeConstraint {
    pub agent: usize,
    pub from: IVec2,
    pub to: IVec2,
    pub time: u64,
}

#[derive(Debug, Clone)]
pub struct Constraints {
    pub vertex: Vec<VertexConstraint>,
    pub edge: Vec<EdgeConstraint>,
}

impl Default for Constraints {
    fn default() -> Self {
        Self::new()
    }
}

impl Constraints {
    pub fn new() -> Self {
        Self {
            vertex: Vec::new(),
            edge: Vec::new(),
        }
    }

    /// Build O(1)-lookup index for a specific agent.
    fn index_for(&self, agent: usize) -> ConstraintIndex {
        let mut vertex_set =
            HashSet::with_capacity(self.vertex.len());
        for c in &self.vertex {
            if c.agent == agent {
                vertex_set.insert((c.pos, c.time));
            }
        }
        let mut edge_set =
            HashSet::with_capacity(self.edge.len());
        for c in &self.edge {
            if c.agent == agent {
                edge_set.insert((c.from, c.to, c.time));
            }
        }
        ConstraintIndex {
            vertex: vertex_set,
            edge: edge_set,
        }
    }
}

// ---------------------------------------------------------------------------
// ConstraintChecker — generic constraint interface
// ---------------------------------------------------------------------------

/// Trait for checking spacetime constraints during A* expansion.
/// Implemented by both HashSet-based ConstraintIndex and flat FlatConstraintIndex.
pub trait ConstraintChecker {
    fn is_vertex_blocked(&self, pos: IVec2, time: u64) -> bool;
    fn is_edge_blocked(&self, from: IVec2, to: IVec2, time: u64) -> bool;
}

// ---------------------------------------------------------------------------
// ConstraintIndex (HashSet-based, original implementation)
// ---------------------------------------------------------------------------

/// Pre-indexed constraints for O(1) lookups during A* expansion.
/// Can be built incrementally via `add_vertex`/`add_edge` for sequential planners.
pub struct ConstraintIndex {
    vertex: HashSet<(IVec2, u64)>,
    edge: HashSet<(IVec2, IVec2, u64)>,
}

impl Default for ConstraintIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstraintIndex {
    pub fn new() -> Self {
        Self {
            vertex: HashSet::new(),
            edge: HashSet::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            vertex: HashSet::with_capacity(cap),
            edge: HashSet::with_capacity(cap),
        }
    }

    #[inline]
    pub fn add_vertex(&mut self, pos: IVec2, time: u64) {
        self.vertex.insert((pos, time));
    }

    #[inline]
    pub fn add_edge(&mut self, from: IVec2, to: IVec2, time: u64) {
        self.edge.insert((from, to, time));
    }
}

impl ConstraintChecker for ConstraintIndex {
    #[inline]
    fn is_vertex_blocked(&self, pos: IVec2, time: u64) -> bool {
        self.vertex.contains(&(pos, time))
    }

    #[inline]
    fn is_edge_blocked(&self, from: IVec2, to: IVec2, time: u64) -> bool {
        self.edge.contains(&(from, to, time))
    }
}

// ---------------------------------------------------------------------------
// FlatConstraintIndex — Vec<bool>-based, O(1) with zero hashing
// ---------------------------------------------------------------------------

/// Map a movement delta to a direction ordinal for edge constraint indexing.
/// North(0,1)=0, South(0,-1)=1, East(1,0)=2, West(-1,0)=3, Self(0,0)=4
#[inline]
fn dir_ordinal(from: IVec2, to: IVec2) -> usize {
    let d = to - from;
    match (d.x, d.y) {
        (0, 1) => 0,
        (0, -1) => 1,
        (1, 0) => 2,
        (-1, 0) => 3,
        _ => 4,
    }
}

/// Flat-array constraint index for O(1) lookups with zero hashing.
///
/// Vertex constraints indexed by `(y * width + x) * stride + time`.
/// Edge constraints indexed by `((y * width + x) * 5 + dir) * stride + time`.
pub struct FlatConstraintIndex {
    vertex: Vec<bool>,
    edge: Vec<bool>,
    width: i32,
    stride: usize,
    cells: usize,
}

impl FlatConstraintIndex {
    pub fn new(width: i32, height: i32, max_time: u64) -> Self {
        let stride = (max_time + 1) as usize;
        let cells = (width * height) as usize;
        Self {
            vertex: vec![false; cells * stride],
            edge: vec![false; cells * 5 * stride],
            width,
            stride,
            cells,
        }
    }

    pub fn reset(&mut self, width: i32, height: i32, max_time: u64) {
        let stride = (max_time + 1) as usize;
        let cells = (width * height) as usize;
        let vtotal = cells * stride;
        let etotal = cells * 5 * stride;

        if self.vertex.len() != vtotal || self.width != width {
            self.width = width;
            self.stride = stride;
            self.cells = cells;
            self.vertex = vec![false; vtotal];
            self.edge = vec![false; etotal];
        } else {
            self.vertex.fill(false);
            self.edge.fill(false);
        }
    }

    #[inline]
    fn vertex_idx(&self, pos: IVec2, time: u64) -> usize {
        (pos.y * self.width + pos.x) as usize * self.stride + time as usize
    }

    #[inline]
    fn edge_idx(&self, from: IVec2, to: IVec2, time: u64) -> usize {
        let pos_flat = (from.y * self.width + from.x) as usize;
        let dir = dir_ordinal(from, to);
        (pos_flat * 5 + dir) * self.stride + time as usize
    }

    #[inline]
    pub fn add_vertex(&mut self, pos: IVec2, time: u64) {
        let idx = self.vertex_idx(pos, time);
        if idx < self.vertex.len() {
            self.vertex[idx] = true;
        }
    }

    #[inline]
    pub fn add_edge(&mut self, from: IVec2, to: IVec2, time: u64) {
        let idx = self.edge_idx(from, to, time);
        if idx < self.edge.len() {
            self.edge[idx] = true;
        }
    }
}

impl ConstraintChecker for FlatConstraintIndex {
    #[inline]
    fn is_vertex_blocked(&self, pos: IVec2, time: u64) -> bool {
        let idx = self.vertex_idx(pos, time);
        idx < self.vertex.len() && self.vertex[idx]
    }

    #[inline]
    fn is_edge_blocked(&self, from: IVec2, to: IVec2, time: u64) -> bool {
        let idx = self.edge_idx(from, to, time);
        idx < self.edge.len() && self.edge[idx]
    }
}

// ---------------------------------------------------------------------------
// SpacetimeGrid — reusable flat-array storage for A*
// ---------------------------------------------------------------------------

const NO_PARENT: usize = usize::MAX;

#[derive(Clone, Copy)]
struct CameFromEntry {
    parent_st_idx: usize,
    action: Action,
}

impl CameFromEntry {
    const EMPTY: Self = Self {
        parent_st_idx: NO_PARENT,
        action: Action::Wait,
    };
}

/// Reusable flat-array storage for spacetime A*, replacing HashMap/HashSet.
///
/// Indexed by `(y * width + x) * stride + time`. Cleared with `fill()` on
/// each reset. For typical warehouse grids (840 cells × 41 timesteps = 34K
/// entries), reset takes ~1µs.
pub struct SpacetimeGrid {
    width: i32,
    stride: usize,
    total: usize,
    best_g: Vec<u64>,
    came_from: Vec<CameFromEntry>,
    closed: Vec<bool>,
    /// Reusable buffer for path reconstruction (avoids per-call Vec allocation).
    path_buf: Vec<Action>,
}

impl Default for SpacetimeGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl SpacetimeGrid {
    pub fn new() -> Self {
        Self {
            width: 0,
            stride: 0,
            total: 0,
            best_g: Vec::new(),
            came_from: Vec::new(),
            closed: Vec::new(),
            path_buf: Vec::new(),
        }
    }

    /// Prepare for a new A* search. Resizes if dimensions changed, otherwise
    /// clears with `fill()`.
    pub fn reset(&mut self, width: i32, height: i32, max_time: u64) {
        let stride = (max_time + 1) as usize;
        let total = (width * height) as usize * stride;

        self.width = width;
        self.stride = stride;

        if self.total != total {
            self.total = total;
            self.best_g = vec![u64::MAX; total];
            self.came_from = vec![CameFromEntry::EMPTY; total];
            self.closed = vec![false; total];
        } else {
            self.best_g.fill(u64::MAX);
            // came_from: stale entries are never reached during reconstruction
            // (the chain only follows entries set during this run), so skip clearing.
            self.closed.fill(false);
        }
    }

    #[inline]
    fn st_index(&self, pos: IVec2, time: u64) -> usize {
        (pos.y * self.width + pos.x) as usize * self.stride + time as usize
    }
}

// ---------------------------------------------------------------------------
// Node (shared by both A* variants)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Eq, PartialEq)]
struct Node {
    pos: IVec2,
    time: u64,
    g: u64,
    f: u64,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.cmp(&self.f).then_with(|| other.g.cmp(&self.g))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Spacetime A* — fast path (flat arrays + generic constraints)
// ---------------------------------------------------------------------------

/// High-performance spacetime A* using flat arrays and generic constraint checking.
///
/// Uses `SpacetimeGrid` for O(1) array lookups instead of HashMap. The constraint
/// source is generic: works with `ConstraintIndex`, `FlatConstraintIndex`, or any
/// type implementing `ConstraintChecker` (e.g., Token Passing's MasterConstraintIndex).
///
/// The caller owns the `SpacetimeGrid` and reuses it across calls to avoid allocation.
pub fn spacetime_astar_fast<C: ConstraintChecker>(
    grid: &GridMap,
    start: IVec2,
    goal: IVec2,
    ci: &C,
    max_time: u64,
    dist_map: Option<&DistanceMap>,
    stg: &mut SpacetimeGrid,
    max_expansions: u64,
) -> Result<Vec<Action>, SolverError> {
    if !grid.is_walkable(start) || !grid.is_walkable(goal) {
        return Err(SolverError::InvalidInput(format!(
            "start {start} or goal {goal} is not walkable"
        )));
    }

    stg.reset(grid.width, grid.height, max_time);

    let heuristic = |pos: IVec2| -> u64 {
        match dist_map {
            Some(dm) => {
                let d = dm.get(pos);
                if d == u64::MAX { manhattan(pos, goal) } else { d }
            }
            None => manhattan(pos, goal),
        }
    };

    let start_st = stg.st_index(start, 0);
    stg.best_g[start_st] = 0;

    let h = heuristic(start);
    let mut open: BinaryHeap<Node> = BinaryHeap::with_capacity(256);
    open.push(Node { pos: start, time: 0, g: 0, f: h });
    let mut expansions: u64 = 0;

    while let Some(current) = open.pop() {
        if current.pos == goal {
            // Reconstruct path into reusable path_buf (avoids growth reallocs).
            // path_buf retains capacity across calls; only the final to_vec()
            // allocates the exact-size result.
            stg.path_buf.clear();
            let goal_st = stg.st_index(current.pos, current.time);
            let mut cur_st = goal_st;
            while cur_st != start_st {
                let entry = stg.came_from[cur_st];
                if entry.parent_st_idx == NO_PARENT {
                    break;
                }
                stg.path_buf.push(entry.action);
                cur_st = entry.parent_st_idx;
            }
            stg.path_buf.reverse();
            return Ok(stg.path_buf.to_vec());
        }

        if current.time >= max_time {
            continue;
        }

        let cur_st = stg.st_index(current.pos, current.time);

        if stg.closed[cur_st] {
            continue;
        }
        stg.closed[cur_st] = true;
        expansions += 1;
        if expansions >= max_expansions {
            return Err(SolverError::NoSolution);
        }

        let next_time = current.time + 1;

        // Wait action
        if !ci.is_vertex_blocked(current.pos, next_time)
            && !ci.is_edge_blocked(current.pos, current.pos, current.time)
        {
            let g = current.g + 1;
            let next_st = stg.st_index(current.pos, next_time);
            if !stg.closed[next_st] && g < stg.best_g[next_st] {
                stg.best_g[next_st] = g;
                stg.came_from[next_st] = CameFromEntry {
                    parent_st_idx: cur_st,
                    action: Action::Wait,
                };
                let f = g + heuristic(current.pos);
                open.push(Node { pos: current.pos, time: next_time, g, f });
            }
        }

        // Move actions
        for dir in Direction::ALL {
            let next_pos = current.pos + dir.offset();

            if !grid.is_walkable(next_pos) {
                continue;
            }
            if ci.is_vertex_blocked(next_pos, next_time) {
                continue;
            }
            if ci.is_edge_blocked(current.pos, next_pos, current.time) {
                continue;
            }

            let g = current.g + 1;
            let next_st = stg.st_index(next_pos, next_time);
            if stg.closed[next_st] {
                continue;
            }
            if g >= stg.best_g[next_st] {
                continue;
            }

            stg.best_g[next_st] = g;
            stg.came_from[next_st] = CameFromEntry {
                parent_st_idx: cur_st,
                action: Action::Move(dir),
            };
            let f = g + heuristic(next_pos);
            open.push(Node { pos: next_pos, time: next_time, g, f });
        }
    }

    Err(SolverError::NoSolution)
}

// ---------------------------------------------------------------------------
// Spacetime A* — original path (HashMap-based, kept for backward compatibility)
// ---------------------------------------------------------------------------

/// Spacetime A* with optional BFS-based heuristic.
///
/// When `dist_map` is `Some`, uses precomputed BFS distance (tighter, fewer
/// expansions). Falls back to Manhattan when `None`.
pub fn spacetime_astar(
    grid: &GridMap,
    start: IVec2,
    goal: IVec2,
    agent: usize,
    constraints: &Constraints,
    max_time: u64,
) -> Result<Vec<Action>, SolverError> {
    spacetime_astar_guided(grid, start, goal, agent, constraints, max_time, None)
}

/// Spacetime A* with BFS-guided heuristic for fewer node expansions.
pub fn spacetime_astar_guided(
    grid: &GridMap,
    start: IVec2,
    goal: IVec2,
    agent: usize,
    constraints: &Constraints,
    max_time: u64,
    dist_map: Option<&DistanceMap>,
) -> Result<Vec<Action>, SolverError> {
    let ci = constraints.index_for(agent);
    spacetime_astar_with_index(grid, start, goal, &ci, max_time, dist_map)
}

/// Spacetime A* with a pre-built constraint index.
/// Use this when the caller maintains an incremental index (avoids rebuilding).
pub fn spacetime_astar_with_index(
    grid: &GridMap,
    start: IVec2,
    goal: IVec2,
    ci: &ConstraintIndex,
    max_time: u64,
    dist_map: Option<&DistanceMap>,
) -> Result<Vec<Action>, SolverError> {
    if !grid.is_walkable(start) || !grid.is_walkable(goal) {
        return Err(SolverError::InvalidInput(format!(
            "start {start} or goal {goal} is not walkable"
        )));
    }

    let heuristic = |pos: IVec2| -> u64 {
        match dist_map {
            Some(dm) => {
                let d = dm.get(pos);
                if d == u64::MAX { manhattan(pos, goal) } else { d }
            }
            None => manhattan(pos, goal),
        }
    };

    let h = heuristic(start);
    let start_node = Node {
        pos: start,
        time: 0,
        g: 0,
        f: h,
    };

    let cap = (max_time as usize) * 8;
    let mut open: BinaryHeap<Node> = BinaryHeap::with_capacity(cap.min(256));
    let mut closed: HashSet<(IVec2, u64)> = HashSet::with_capacity(cap.min(256));
    let mut came_from: HashMap<(IVec2, u64), (IVec2, u64, Action)> =
        HashMap::with_capacity(cap.min(256));
    let mut best_g: HashMap<(IVec2, u64), u64> = HashMap::with_capacity(cap.min(256));

    open.push(start_node);
    best_g.insert((start, 0), 0);

    while let Some(current) = open.pop() {
        if current.pos == goal {
            return Ok(reconstruct_path(
                &came_from,
                start,
                current.pos,
                current.time,
            ));
        }

        if current.time >= max_time {
            continue;
        }

        if !closed.insert((current.pos, current.time)) {
            continue;
        }

        let next_time = current.time + 1;

        // Wait action
        if !ci.is_vertex_blocked(current.pos, next_time)
            && !ci.is_edge_blocked(current.pos, current.pos, current.time)
        {
            let g = current.g + 1;
            let key = (current.pos, next_time);
            if !closed.contains(&key) && g < *best_g.get(&key).unwrap_or(&u64::MAX) {
                best_g.insert(key, g);
                came_from.insert(key, (current.pos, current.time, Action::Wait));
                let f = g + heuristic(current.pos);
                open.push(Node {
                    pos: current.pos,
                    time: next_time,
                    g,
                    f,
                });
            }
        }

        // Move actions
        for dir in Direction::ALL {
            let next_pos = current.pos + dir.offset();

            if !grid.is_walkable(next_pos) {
                continue;
            }
            if ci.is_vertex_blocked(next_pos, next_time) {
                continue;
            }
            if ci.is_edge_blocked(current.pos, next_pos, current.time) {
                continue;
            }

            let g = current.g + 1;
            let key = (next_pos, next_time);
            if closed.contains(&key) {
                continue;
            }
            if g >= *best_g.get(&key).unwrap_or(&u64::MAX) {
                continue;
            }

            best_g.insert(key, g);
            came_from.insert(key, (current.pos, current.time, Action::Move(dir)));
            let f = g + heuristic(next_pos);
            open.push(Node {
                pos: next_pos,
                time: next_time,
                g,
                f,
            });
        }
    }

    Err(SolverError::NoSolution)
}

fn reconstruct_path(
    came_from: &HashMap<(IVec2, u64), (IVec2, u64, Action)>,
    start: IVec2,
    goal_pos: IVec2,
    goal_time: u64,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut current = (goal_pos, goal_time);

    while current != (start, 0) {
        if let Some(&(parent_pos, parent_time, action)) = came_from.get(&current) {
            actions.push(action);
            current = (parent_pos, parent_time);
        } else {
            break;
        }
    }

    actions.reverse();
    actions
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::solver::heuristics::DistanceMap;

    #[test]
    fn flat_constraint_matches_hashset() {
        let w = 10;
        let h = 10;
        let max_time = 20u64;

        let mut ci = ConstraintIndex::new();
        let mut fci = FlatConstraintIndex::new(w, h, max_time);

        // Add some vertex constraints
        let vertices = [
            (IVec2::new(3, 4), 5u64),
            (IVec2::new(0, 0), 0),
            (IVec2::new(9, 9), 20),
            (IVec2::new(5, 5), 10),
        ];
        for &(pos, t) in &vertices {
            ci.add_vertex(pos, t);
            fci.add_vertex(pos, t);
        }

        // Add some edge constraints
        let edges = [
            (IVec2::new(3, 4), IVec2::new(3, 5), 5u64),  // North
            (IVec2::new(0, 0), IVec2::new(1, 0), 0),      // East
            (IVec2::new(5, 5), IVec2::new(5, 5), 10),      // Self/Wait
            (IVec2::new(7, 3), IVec2::new(6, 3), 15),      // West
        ];
        for &(from, to, t) in &edges {
            ci.add_edge(from, to, t);
            fci.add_edge(from, to, t);
        }

        // Verify all positions × all times give identical results
        for x in 0..w {
            for y in 0..h {
                let pos = IVec2::new(x, y);
                for t in 0..=max_time {
                    assert_eq!(
                        ci.is_vertex_blocked(pos, t),
                        fci.is_vertex_blocked(pos, t),
                        "vertex mismatch at ({x},{y}) t={t}"
                    );
                }
            }
        }

        // Verify edge constraints match
        for &(from, to, t) in &edges {
            assert_eq!(
                ci.is_edge_blocked(from, to, t),
                fci.is_edge_blocked(from, to, t),
                "edge mismatch at {from}->{to} t={t}"
            );
        }

        // Verify non-constrained edges are not blocked
        assert!(!fci.is_edge_blocked(IVec2::new(0, 0), IVec2::new(0, 1), 5));
        assert!(!ci.is_edge_blocked(IVec2::new(0, 0), IVec2::new(0, 1), 5));
    }

    #[test]
    fn flat_constraint_reset_clears() {
        let mut fci = FlatConstraintIndex::new(5, 5, 10);
        fci.add_vertex(IVec2::new(2, 2), 5);
        assert!(fci.is_vertex_blocked(IVec2::new(2, 2), 5));

        fci.reset(5, 5, 10);
        assert!(!fci.is_vertex_blocked(IVec2::new(2, 2), 5));
    }

    #[test]
    fn spacetime_grid_basic() {
        let mut stg = SpacetimeGrid::new();
        stg.reset(5, 5, 10);

        let idx = stg.st_index(IVec2::new(2, 3), 5);
        assert_eq!(stg.best_g[idx], u64::MAX);
        assert!(!stg.closed[idx]);

        stg.best_g[idx] = 7;
        stg.closed[idx] = true;
        assert_eq!(stg.best_g[idx], 7);
        assert!(stg.closed[idx]);

        // Reset should clear
        stg.reset(5, 5, 10);
        assert_eq!(stg.best_g[idx], u64::MAX);
        assert!(!stg.closed[idx]);
    }

    #[test]
    fn fast_astar_matches_original() {
        let grid = GridMap::new(10, 10);
        let start = IVec2::new(0, 0);
        let goal = IVec2::new(9, 9);
        let dm = DistanceMap::compute(&grid, goal);

        let ci = ConstraintIndex::new();
        let original = spacetime_astar_with_index(
            &grid, start, goal, &ci, 30, Some(&dm),
        ).unwrap();

        let mut stg = SpacetimeGrid::new();
        let fast = spacetime_astar_fast(
            &grid, start, goal, &ci, 30, Some(&dm), &mut stg, u64::MAX,
        ).unwrap();

        // Both should find optimal path of length 18 (Manhattan distance)
        assert_eq!(original.len(), fast.len());
        // Verify both reach the goal
        let mut pos = start;
        for a in &fast {
            pos = a.apply(pos);
        }
        assert_eq!(pos, goal);
    }

    #[test]
    fn fast_astar_with_constraints() {
        let grid = GridMap::new(5, 5);
        let start = IVec2::new(0, 0);
        let goal = IVec2::new(4, 0);

        // Block the direct path at time 2
        let mut ci = ConstraintIndex::new();
        ci.add_vertex(IVec2::new(2, 0), 2);

        let original = spacetime_astar_with_index(
            &grid, start, goal, &ci, 20, None,
        ).unwrap();

        let mut stg = SpacetimeGrid::new();
        let fast = spacetime_astar_fast(
            &grid, start, goal, &ci, 20, None, &mut stg, u64::MAX,
        ).unwrap();

        assert_eq!(original.len(), fast.len());

        // Verify both reach goal
        let mut pos = start;
        for a in &fast {
            pos = a.apply(pos);
        }
        assert_eq!(pos, goal);
    }

    #[test]
    fn fast_astar_with_flat_constraints() {
        let grid = GridMap::new(5, 5);
        let start = IVec2::new(0, 0);
        let goal = IVec2::new(4, 0);

        // Same constraint using flat index
        let mut fci = FlatConstraintIndex::new(5, 5, 20);
        fci.add_vertex(IVec2::new(2, 0), 2);

        let mut stg = SpacetimeGrid::new();
        let result = spacetime_astar_fast(
            &grid, start, goal, &fci, 20, None, &mut stg, u64::MAX,
        ).unwrap();

        let mut pos = start;
        for a in &result {
            pos = a.apply(pos);
        }
        assert_eq!(pos, goal);
    }

    #[test]
    fn fast_astar_no_solution() {
        let mut grid = GridMap::new(5, 5);
        // Wall off the goal
        grid.set_obstacle(IVec2::new(3, 0));
        grid.set_obstacle(IVec2::new(3, 1));
        grid.set_obstacle(IVec2::new(3, 2));
        grid.set_obstacle(IVec2::new(3, 3));
        grid.set_obstacle(IVec2::new(3, 4));

        let ci = ConstraintIndex::new();
        let mut stg = SpacetimeGrid::new();
        let result = spacetime_astar_fast(
            &grid, IVec2::ZERO, IVec2::new(4, 0), &ci, 20, None, &mut stg, u64::MAX,
        );
        assert!(result.is_err());
    }

    #[test]
    fn spacetime_grid_reuse_across_calls() {
        let grid = GridMap::new(5, 5);
        let ci = ConstraintIndex::new();
        let mut stg = SpacetimeGrid::new();

        // First call
        let r1 = spacetime_astar_fast(
            &grid, IVec2::ZERO, IVec2::new(4, 4), &ci, 20, None, &mut stg, u64::MAX,
        ).unwrap();

        // Second call with same grid (reuses arrays)
        let r2 = spacetime_astar_fast(
            &grid, IVec2::new(4, 0), IVec2::new(0, 4), &ci, 20, None, &mut stg, u64::MAX,
        ).unwrap();

        // Both should find valid paths
        let mut pos = IVec2::ZERO;
        for a in &r1 { pos = a.apply(pos); }
        assert_eq!(pos, IVec2::new(4, 4));

        let mut pos = IVec2::new(4, 0);
        for a in &r2 { pos = a.apply(pos); }
        assert_eq!(pos, IVec2::new(0, 4));
    }
}
