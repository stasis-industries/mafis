//! LaCAM3 Instance — adaptation of lacam3/instance.cpp + graph.cpp.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/instance.cpp
//!            docs/papers_codes/lacam3/lacam3/src/graph.cpp
//!            docs/papers_codes/lacam3/lacam3/include/graph.hpp
//!            docs/papers_codes/lacam3/lacam3/include/instance.hpp
//!
//! ## Adaptations to MAFIS
//!
//! lacam3's `Vertex { id, index, x, y, neighbor }` and `Graph { V, U, width,
//! height }` are replaced by direct use of MAFIS's `GridMap` (which already
//! provides `walkable_neighbors()` at `IVec2` coordinates) plus a
//! flat-cell-id index `y * width + x` for fast lookup.
//!
//! lacam3 uses `Config = vector<Vertex*>`. We use `Config = Vec<u32>` where
//! each element is a flat cell id (matching `Vertex::id` semantics). This
//! gives O(1) hashing for the EXPLORED map and avoids pointer chasing.

use bevy::prelude::*;
use crate::core::grid::GridMap;

/// A configuration: one cell per agent. Indexed by agent id.
///
/// REFERENCE: lacam3 graph.hpp line 17 `using Config = vector<Vertex*>`.
/// MAFIS uses flat cell ids (`u32`) for hashability and zero allocation.
pub type Config = Vec<u32>;

/// A path: sequence of cells. Indexed by timestep.
///
/// REFERENCE: lacam3 graph.hpp line 18 `using Path = vector<Vertex*>`.
pub type Path = Vec<u32>;

/// Solution: a sequence of full configurations.
///
/// REFERENCE: lacam3 instance.hpp line 34 `using Solution = vector<Config>`.
pub type Solution = Vec<Config>;

/// LaCAM3 Instance: graph + start configuration + goal configuration.
///
/// REFERENCE: lacam3 instance.hpp lines 10-31.
pub struct Instance<'a> {
    /// MAFIS GridMap (replaces lacam3 `Graph*`)
    pub grid: &'a GridMap,
    /// Initial configuration: starts[i] = flat cell id of agent i's start
    pub starts: Config,
    /// Goal configuration: goals[i] = flat cell id of agent i's goal
    pub goals: Config,
    /// Number of agents
    pub n: usize,
    /// Total number of grid cells (V_size in lacam3)
    pub v_size: usize,
}

impl<'a> Instance<'a> {
    pub fn new(grid: &'a GridMap, starts: Vec<IVec2>, goals: Vec<IVec2>) -> Self {
        debug_assert_eq!(starts.len(), goals.len());
        let n = starts.len();
        let v_size = (grid.width * grid.height) as usize;
        let starts_ids = starts.iter().map(|&p| pos_to_id(p, grid.width)).collect();
        let goals_ids = goals.iter().map(|&p| pos_to_id(p, grid.width)).collect();
        Self { grid, starts: starts_ids, goals: goals_ids, n, v_size }
    }
}

/// Convert IVec2 to flat cell id.
///
/// REFERENCE: lacam3 graph.hpp line 9 `index = width * y + x`.
#[inline]
pub fn pos_to_id(pos: IVec2, width: i32) -> u32 {
    (pos.y * width + pos.x) as u32
}

/// Convert flat cell id back to IVec2.
#[inline]
pub fn id_to_pos(id: u32, width: i32) -> IVec2 {
    let id = id as i32;
    IVec2::new(id % width, id / width)
}

/// Check if two configurations are equal.
///
/// REFERENCE: lacam3 graph.cpp `is_same_config`.
#[inline]
pub fn is_same_config(c1: &Config, c2: &Config) -> bool {
    c1 == c2
}

/// Get walkable neighbors of a cell as flat cell ids.
///
/// REFERENCE: lacam3 graph.hpp line 12 `Vertex::neighbor`.
/// In lacam3 this is precomputed at graph construction; in MAFIS we query
/// the GridMap on demand (still O(1) since at most 4 neighbors).
pub fn neighbors(grid: &GridMap, id: u32) -> smallvec::SmallVec<[u32; 4]> {
    let pos = id_to_pos(id, grid.width);
    grid.walkable_neighbors(pos)
        .into_iter()
        .map(|p| pos_to_id(p, grid.width))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pos_id_roundtrip() {
        let width = 10;
        for x in 0..width {
            for y in 0..15 {
                let pos = IVec2::new(x, y);
                let id = pos_to_id(pos, width);
                assert_eq!(id_to_pos(id, width), pos);
            }
        }
    }

    #[test]
    fn instance_construction() {
        let grid = GridMap::new(5, 5);
        let starts = vec![IVec2::new(0, 0), IVec2::new(4, 4)];
        let goals = vec![IVec2::new(4, 0), IVec2::new(0, 4)];
        let ins = Instance::new(&grid, starts, goals);
        assert_eq!(ins.n, 2);
        assert_eq!(ins.v_size, 25);
        assert_eq!(ins.starts[0], 0); // (0,0) = 0
        assert_eq!(ins.starts[1], 24); // (4,4) = 4*5+4 = 24
    }
}
