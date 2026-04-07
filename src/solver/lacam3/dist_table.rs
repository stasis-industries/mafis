//! LaCAM3 DistTable — per-agent BFS distance table to goal.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/dist_table.cpp
//!            docs/papers_codes/lacam3/lacam3/include/dist_table.hpp
//!
//! ## Adaptations to MAFIS
//!
//! lacam3's `DistTable` lazily computes per-agent BFS from each goal cell.
//! MAFIS already has `crate::solver::shared::heuristics::DistanceMap` and
//! `DistanceMapCache` that do exactly this — but they cache by goal cell
//! rather than by agent index. We adapt by computing one `DistanceMap` per
//! agent and storing them in a flat vector.
//!
//! lacam3 uses `std::async` to compute all BFS in parallel. WASM-compatible
//! MAFIS computes them sequentially (small overhead since BFS is fast).
//!
//! Returns `K` (the v_size) for unreachable cells, matching lacam3 line 4
//! `table(ins.N, vector<int>(K, K))`.

use crate::core::grid::GridMap;
use crate::solver::shared::heuristics::DistanceMap;
use super::instance::{Instance, id_to_pos};

/// Per-agent distance table.
///
/// REFERENCE: lacam3 dist_table.hpp lines 10-23.
pub struct DistTable {
    /// One DistanceMap per agent. `maps[i].get(pos)` gives BFS distance from
    /// `pos` to agent `i`'s goal.
    maps: Vec<DistanceMap>,
    /// Total cell count, used as the "unreachable" sentinel value.
    /// REFERENCE: lacam3 dist_table.cpp line 4 `K = ins.G->V.size()`.
    pub k: usize,
    /// Grid width for cell-id → IVec2 conversion.
    width: i32,
}

impl DistTable {
    /// Build the distance table by running BFS from each agent's goal.
    ///
    /// REFERENCE: lacam3 dist_table.cpp `setup` lines 15-38.
    pub fn new(grid: &GridMap, ins: &Instance) -> Self {
        let mut maps = Vec::with_capacity(ins.n);
        for &goal_id in &ins.goals {
            let goal = id_to_pos(goal_id, grid.width);
            maps.push(DistanceMap::compute(grid, goal));
        }
        Self { maps, k: ins.v_size, width: grid.width }
    }

    /// Get BFS distance from cell `v_id` to agent `i`'s goal.
    /// Returns `k` (sentinel) if unreachable.
    ///
    /// REFERENCE: lacam3 dist_table.cpp `get` line 40.
    #[inline]
    pub fn get(&self, i: usize, v_id: u32) -> i32 {
        let pos = id_to_pos(v_id, self.width);
        let d = self.maps[i].get(pos);
        if d == u64::MAX || d as usize >= self.k {
            self.k as i32
        } else {
            d as i32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn dist_table_basic() {
        let grid = GridMap::new(5, 5);
        let starts = vec![IVec2::new(0, 0), IVec2::new(4, 4)];
        let goals = vec![IVec2::new(4, 4), IVec2::new(0, 0)];
        let ins = Instance::new(&grid, starts, goals);
        let dt = DistTable::new(&grid, &ins);

        // Agent 0 goal = (4,4). Distance from (4,4) to itself = 0.
        let goal_id_0 = super::super::instance::pos_to_id(IVec2::new(4, 4), 5);
        assert_eq!(dt.get(0, goal_id_0), 0);

        // Distance from (0,0) to (4,4) on open grid = 8 (Manhattan)
        let start_id = super::super::instance::pos_to_id(IVec2::new(0, 0), 5);
        assert_eq!(dt.get(0, start_id), 8);

        // Agent 1 goal = (0,0). Distance from (4,4) to (0,0) = 8.
        let start_id_1 = super::super::instance::pos_to_id(IVec2::new(4, 4), 5);
        assert_eq!(dt.get(1, start_id_1), 8);
    }
}
