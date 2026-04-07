//! LaCAM3 Scatter — SUO (Space Utilization Optimization) heuristic.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/scatter.cpp (147 lines)
//!            docs/papers_codes/lacam3/lacam3/include/scatter.hpp
//!
//! Diversifies path proposals to escape symmetric local minima. This is the
//! key engineering feature that distinguishes lacam3 from lacam2 — without
//! it the implementation reverts to lacam2 behavior.
//!
//! ## Algorithm (collision-aware single-agent A*)
//!
//! 1. T = makespan_lower_bound + cost_margin
//! 2. Loop while collisions decrease (and not expired):
//!    a. Shuffle planning order
//!    b. For each agent i in shuffled order:
//!       - Remove agent i's current path from the CollisionTable
//!       - Run single-agent A* with f = (collision_cost, g+h, vertex_id)
//!         (lexicographic — prefer fewer collisions, tie-break on f-value)
//!       - Re-enroll new path in CollisionTable
//! 3. Build scatter_data: for each agent's path, map cell → next cell
//!
//! ## Adaptations
//!
//! - lacam3 has a Deadline parameter for time-bounded scattering. MAFIS
//!   uses an iteration-count budget instead (since wall-clock varies in
//!   WASM). Set via `MAX_ITERS`.
//! - The single-agent A* uses a `BinaryHeap` instead of `priority_queue`.

use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::cmp::Ordering;

use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use rand::seq::SliceRandom;

use super::collision_table::CollisionTable;
use super::dist_table::DistTable;
use super::instance::{Instance, Path, neighbors};

/// Maximum scatter loop iterations (analog of lacam3's deadline).
pub const SCATTER_MAX_ITERS: usize = 10;

/// SUO heuristic: per-agent map from "current cell" → "preferred next cell".
///
/// REFERENCE: lacam3 scatter.hpp lines 17-41.
pub struct Scatter {
    /// `scatter_data[agent][current_v_id] = preferred_next_v_id`.
    /// REFERENCE: lacam3 scatter.hpp line 32.
    pub scatter_data: Vec<HashMap<u32, u32>>,
    /// Computed paths (kept for inspection / metrics).
    pub paths: Vec<Path>,
}

impl Scatter {
    /// Construct an empty scatter (no preferences).
    pub fn empty(n: usize) -> Self {
        Self {
            scatter_data: vec![HashMap::new(); n],
            paths: vec![Vec::new(); n],
        }
    }

    /// Get the preferred next cell for agent `i` at cell `v`, if any.
    ///
    /// REFERENCE: lacam3 pibt.cpp lines 73-77 — used inside `funcPIBT`
    /// to bias the candidate ordering.
    pub fn get_next(&self, i: usize, v: u32) -> Option<u32> {
        self.scatter_data.get(i).and_then(|m| m.get(&v).copied())
    }

    /// Run the SUO loop and produce scatter data.
    ///
    /// REFERENCE: lacam3 scatter.cpp `Scatter::construct` lines 23-147.
    pub fn construct(
        ins: &Instance,
        d: &DistTable,
        seed: u64,
        cost_margin: i32,
    ) -> Self {
        let n = ins.n;
        let v_size = ins.v_size;
        let mut rng = SeededRng::new(seed);

        // Compute makespan lower bound = max over agents of dist(start_i, goal_i).
        // REFERENCE: scatter.cpp line 13 + metrics.cpp line 98.
        let mut t_lb = 0i32;
        for i in 0..n {
            let d_i = d.get(i, ins.starts[i]);
            if d_i > t_lb {
                t_lb = d_i;
            }
        }
        let _t = t_lb + cost_margin;

        let mut paths: Vec<Path> = vec![Vec::new(); n];
        let mut ct = CollisionTable::new(ins);

        // Main loop.
        // REFERENCE: scatter.cpp lines 46-134.
        let mut loop_idx = 0;
        let mut collision_cnt_last = i32::MAX;
        let mut paths_prev: Vec<Path> = vec![Vec::new(); n];
        loop {
            if loop_idx >= 2 && ct.collision_cnt >= collision_cnt_last {
                break;
            }
            if loop_idx >= SCATTER_MAX_ITERS {
                break;
            }
            loop_idx += 1;
            collision_cnt_last = ct.collision_cnt;

            // Randomize planning order.
            // REFERENCE: scatter.cpp lines 51-54.
            let mut order: Vec<usize> = (0..n).collect();
            order.shuffle(&mut rng.rng);

            for &i in &order {
                let cost_ub = d.get(i, ins.starts[i]) + cost_margin;

                // Clear current path.
                ct.clear_path(i as u32, &paths[i]);

                // Single-agent A* with collision-aware priority.
                let new_path = scatter_astar(ins.grid, d, &ct, ins.starts[i], ins.goals[i], i, cost_ub, v_size);

                // Replace path.
                paths[i] = new_path;
                ct.enroll_path(i as u32, &paths[i]);
            }

            paths_prev.clone_from(&paths);
            if ct.collision_cnt == 0 {
                break;
            }
        }

        // Set scatter data: for each agent's path, map (cell → next cell).
        // REFERENCE: scatter.cpp lines 138-144.
        let mut scatter_data = vec![HashMap::new(); n];
        for i in 0..n {
            if paths[i].is_empty() {
                continue;
            }
            let p = &paths[i];
            for t in 0..p.len() - 1 {
                scatter_data[i].insert(p[t], p[t + 1]);
            }
        }

        Self { scatter_data, paths }
    }
}

/// Single-agent A* with collision-aware priority.
///
/// Returns the path (or empty if unsolvable within `cost_ub`).
///
/// REFERENCE: lacam3 scatter.cpp lines 68-126.
fn scatter_astar(
    grid: &GridMap,
    d: &DistTable,
    ct: &CollisionTable,
    s_i: u32,
    g_i: u32,
    i: usize,
    cost_ub: i32,
    v_size: usize,
) -> Path {
    /// ScatterNode = (vertex, cost-to-come, cost-to-go, collision_count, parent_vertex).
    /// REFERENCE: scatter.cpp line 29.
    #[derive(Clone, Eq, PartialEq)]
    struct Node {
        v: u32,
        g_v: i32,
        h_v: i32,
        c_v: i32, // collision count
        parent: Option<u32>,
    }

    impl Ord for Node {
        fn cmp(&self, other: &Self) -> Ordering {
            // BinaryHeap is max-heap, so we negate to get min-heap behavior.
            // REFERENCE: scatter.cpp lines 30-38 — comparator returns true if
            // a should be POPPED LATER than b. Order:
            //   1. fewer collisions wins (smaller c_v ranks first)
            //   2. tie-break on smaller f = g + h
            //   3. tie-break on smaller vertex id
            other.c_v.cmp(&self.c_v).then_with(|| {
                let f_self = self.g_v + self.h_v;
                let f_other = other.g_v + other.h_v;
                f_other.cmp(&f_self).then_with(|| other.v.cmp(&self.v))
            })
        }
    }

    impl PartialOrd for Node {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    let mut open: BinaryHeap<Node> = BinaryHeap::new();
    let mut closed: Vec<Option<u32>> = vec![None; v_size]; // parent vertex per cell
    let mut closed_set: Vec<bool> = vec![false; v_size];

    open.push(Node { v: s_i, g_v: 0, h_v: d.get(i, s_i), c_v: 0, parent: None });

    let mut solved = false;
    while let Some(node) = open.pop() {
        if closed_set[node.v as usize] {
            continue;
        }
        closed_set[node.v as usize] = true;
        closed[node.v as usize] = node.parent;

        if node.v == g_i {
            solved = true;
            break;
        }

        // Expand neighbors.
        // REFERENCE: scatter.cpp lines 98-106.
        for u in neighbors(grid, node.v) {
            let d_u = d.get(i, u);
            if u != s_i && !closed_set[u as usize] && d_u + node.g_v + 1 <= cost_ub {
                let new_c = ct.get_collision_cost(node.v, u, node.g_v as usize) + node.c_v;
                open.push(Node {
                    v: u,
                    g_v: node.g_v + 1,
                    h_v: d_u,
                    c_v: new_c,
                    parent: Some(node.v),
                });
            }
        }
    }

    // Backtrack.
    // REFERENCE: scatter.cpp lines 110-118.
    if solved {
        let mut path: Vec<u32> = Vec::new();
        let mut v = g_i;
        loop {
            path.push(v);
            match closed[v as usize] {
                Some(parent) => v = parent,
                None => break,
            }
        }
        path.reverse();
        path
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn scatter_empty_is_a_noop() {
        let s = Scatter::empty(3);
        assert_eq!(s.scatter_data.len(), 3);
        assert_eq!(s.get_next(0, 5), None);
    }

    #[test]
    fn scatter_single_agent_finds_direct_path() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let starts = vec![IVec2::new(0, 0)];
        let goals = vec![IVec2::new(4, 0)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);
        let scat = Scatter::construct(&ins, &dt, 42, 2);

        // Single agent should produce a direct path of length 5 (start + 4 moves)
        assert_eq!(scat.paths.len(), 1);
        assert_eq!(scat.paths[0].len(), 5);
        // First cell = start, last = goal
        assert_eq!(scat.paths[0][0], super::super::instance::pos_to_id(IVec2::new(0, 0), 5));
        assert_eq!(*scat.paths[0].last().unwrap(), super::super::instance::pos_to_id(IVec2::new(4, 0), 5));
    }

    #[test]
    fn scatter_two_agents_diversifies_paths() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(7, 7)));
        let starts = vec![IVec2::new(0, 3), IVec2::new(6, 3)];
        let goals = vec![IVec2::new(6, 3), IVec2::new(0, 3)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);
        let scat = Scatter::construct(&ins, &dt, 42, 4);

        // Both agents should produce non-empty paths.
        assert!(!scat.paths[0].is_empty());
        assert!(!scat.paths[1].is_empty());
        // scatter_data should be populated for both
        assert!(!scat.scatter_data[0].is_empty());
        assert!(!scat.scatter_data[1].is_empty());
    }
}
