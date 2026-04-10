//! LaCAM3 HNode — high-level configuration search node.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/hnode.cpp (88 lines)
//!            docs/papers_codes/lacam3/lacam3/include/hnode.hpp
//!
//! An HNode represents one full multi-agent configuration in the search tree.
//! Each HNode has:
//! - `c`: the configuration (one cell per agent)
//! - `parent`: index into the planner's HNode arena
//! - `g, h, f`: cost values
//! - `priorities, order`: per-agent priorities used by PIBT (dynamic priorities; higher value = higher priority)
//! - `search_tree`: queue of LNode constraints to expand for low-level search
//!
//! ## Adaptations
//!
//! lacam3 uses raw pointers (`HNode*`) and a `set<HNode*, CompareHNodePointers>`
//! for the neighbor list. Rust borrow rules make this awkward — instead we
//! use **arena-based storage**: HNodes live in a `Vec<HNode>` owned by the
//! Planner, and references are `usize` indices (`HNodeId`). Neighbors are
//! stored as `Vec<HNodeId>` (sorted for determinism). This pattern is
//! standard for graph search in Rust and avoids unsafe / Rc / lifetimes.
//!
//! `HNode::COUNT` (static counter for instrumentation) is dropped.

use std::collections::VecDeque;

use super::dist_table::DistTable;
use super::instance::Config;
use super::lnode::LNode;
use crate::core::seed::SeededRng;
use rand::seq::SliceRandom;

/// Index into the planner's HNode arena. Replaces `HNode*` from lacam3.
pub type HNodeId = usize;

/// High-level search node.
///
/// REFERENCE: lacam3 hnode.hpp lines 16-39.
#[derive(Debug, Clone)]
pub struct HNode {
    /// The full multi-agent configuration represented by this node.
    pub c: Config,
    /// Parent node id, or `None` for the root.
    pub parent: Option<HNodeId>,
    /// Neighbor node ids (kept sorted for determinism).
    /// REFERENCE: lacam3 hnode.hpp line 21 `set<HNode*, CompareHNodePointers>`.
    pub neighbor: Vec<HNodeId>,

    /// g-value: actual cost from root.
    pub g: i32,
    /// h-value: heuristic estimate to goal.
    pub h: i32,
    /// f = g + h.
    pub f: i32,

    /// Per-agent priorities (dynamic, akin to PIBT). Higher = higher priority.
    /// REFERENCE: lacam3 hnode.hpp line 29 `vector<float> priorities`.
    pub priorities: Vec<f32>,
    /// Agent order (indices sorted by priority desc) — passed to PIBT.
    /// REFERENCE: lacam3 hnode.hpp line 30 `vector<int> order`.
    pub order: Vec<u32>,

    /// FIFO queue of LNode constraints to expand at the low level.
    /// REFERENCE: lacam3 hnode.hpp line 31 `queue<LNode*> search_tree`.
    pub search_tree: VecDeque<LNode>,
}

impl HNode {
    /// Construct a new HNode.
    ///
    /// REFERENCE: lacam3 hnode.cpp lines 7-48 `HNode(C, D, parent, g, h)`.
    pub fn new(
        c: Config,
        d: &DistTable,
        parent_node: Option<&HNode>,
        parent_id: Option<HNodeId>,
        g: i32,
        h: i32,
    ) -> Self {
        let n = c.len();

        let mut search_tree = VecDeque::new();
        // REFERENCE: hnode.cpp line 20 `search_tree.push(new LNode())`.
        search_tree.push_back(LNode::new_root());

        // Set priorities.
        // REFERENCE: hnode.cpp lines 30-42.
        let mut priorities = vec![0.0_f32; n];
        if let Some(p) = parent_node {
            // Dynamic priorities, akin to PIBT (lines 33-42).
            for i in 0..n {
                if d.get(i, c[i]) != 0 {
                    priorities[i] = p.priorities[i] + 1.0;
                } else {
                    // Agent at goal: subtract integer portion (drop the +1 accumulation).
                    priorities[i] = p.priorities[i] - p.priorities[i].trunc();
                }
            }
        } else {
            // Initialize: distance / 10000 as fractional tie-breaker.
            // REFERENCE: hnode.cpp line 32.
            for i in 0..n {
                priorities[i] = (d.get(i, c[i]) as f32) / 10000.0;
            }
        }

        // Set order: agent indices sorted by priority desc.
        // REFERENCE: hnode.cpp lines 44-47.
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by(|&a, &b| {
            priorities[b as usize]
                .partial_cmp(&priorities[a as usize])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Self {
            c,
            parent: parent_id,
            neighbor: Vec::new(),
            g,
            h,
            f: g + h,
            priorities,
            order,
            search_tree,
        }
    }

    /// Expand the next low-level node from the search tree.
    ///
    /// Pops one LNode L from `search_tree`. If `L.depth < N`, generates
    /// successor LNodes for the next-priority agent's neighbors (shuffled
    /// for determinism via the supplied RNG) and pushes them back.
    ///
    /// Returns the popped LNode (the constraint to use for set_new_config),
    /// or None if the search tree is empty.
    ///
    /// REFERENCE: lacam3 hnode.cpp lines 58-72 `get_next_lowlevel_node(MT)`.
    pub fn get_next_lowlevel_node(
        &mut self,
        grid: &crate::core::grid::GridMap,
        rng: &mut SeededRng,
    ) -> Option<LNode> {
        let l = self.search_tree.pop_front()?;
        if l.depth < self.c.len() {
            let i = self.order[l.depth] as usize;
            let agent_pos = self.c[i];
            // Build candidate set: neighbors + self (stay-in-place).
            let raw_neighbors = super::instance::neighbors(grid, agent_pos);
            let mut cands: smallvec::SmallVec<[u32; 5]> = smallvec::SmallVec::new();
            for n in raw_neighbors {
                cands.push(n);
            }
            cands.push(agent_pos);
            // REFERENCE: hnode.cpp line 68 `std::shuffle(cands, MT)`.
            cands.shuffle(&mut rng.rng);
            for u in cands {
                self.search_tree.push_back(LNode::extend(&l, i as u32, u));
            }
        }
        Some(l)
    }
}

/// Compare two HNode configurations lexicographically by cell id.
///
/// REFERENCE: lacam3 hnode.cpp lines 81-88 `CompareHNodePointers`.
/// Used for deterministic neighbor ordering.
pub fn compare_hnode_configs(l: &Config, r: &Config) -> std::cmp::Ordering {
    for (a, b) in l.iter().zip(r.iter()) {
        match a.cmp(b) {
            std::cmp::Ordering::Equal => continue,
            ord => return ord,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::super::instance::{Instance, pos_to_id};
    use super::*;
    use crate::core::grid::GridMap;
    use bevy::prelude::*;

    fn small_instance() -> (GridMap, Instance<'static>) {
        // Leaked grid for static lifetime in test
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let starts = vec![IVec2::new(0, 0), IVec2::new(4, 4)];
        let goals = vec![IVec2::new(4, 4), IVec2::new(0, 0)];
        (grid.clone(), Instance::new(grid, starts, goals))
    }

    #[test]
    fn hnode_root_priorities_are_distance_based() {
        let (_grid_owned, ins) = small_instance();
        let dt = DistTable::new(ins.grid, &ins);
        let hnode = HNode::new(ins.starts.clone(), &dt, None, None, 0, 16);
        // Distance from start to goal = 8 for both agents on open 5×5
        // priorities[i] = 8 / 10000 = 0.0008
        assert!((hnode.priorities[0] - 0.0008).abs() < 1e-6);
        assert!((hnode.priorities[1] - 0.0008).abs() < 1e-6);
    }

    #[test]
    fn hnode_search_tree_has_root_lnode() {
        let (_grid_owned, ins) = small_instance();
        let dt = DistTable::new(ins.grid, &ins);
        let hnode = HNode::new(ins.starts.clone(), &dt, None, None, 0, 16);
        assert_eq!(hnode.search_tree.len(), 1);
        assert_eq!(hnode.search_tree[0].depth, 0);
    }

    #[test]
    fn hnode_get_next_lowlevel_expands_first_agent() {
        let (_grid_owned, ins) = small_instance();
        let dt = DistTable::new(ins.grid, &ins);
        let mut hnode = HNode::new(ins.starts.clone(), &dt, None, None, 0, 16);
        let mut rng = SeededRng::new(42);

        // First call returns the root LNode (depth 0) and pushes children
        // for the next-priority agent's neighbors.
        let l = hnode.get_next_lowlevel_node(ins.grid, &mut rng).unwrap();
        assert_eq!(l.depth, 0);
        // Now search_tree has children: agent at (0,0) has 2 neighbors + self = 3
        // Agent at (4,4) has 2 neighbors + self = 3
        // Whichever has higher priority gets expanded first.
        assert!(hnode.search_tree.len() >= 2);
    }

    #[test]
    fn config_compare_lexicographic() {
        let c1 = vec![1, 2, 3];
        let c2 = vec![1, 2, 4];
        let c3 = vec![1, 2, 3];
        assert_eq!(compare_hnode_configs(&c1, &c2), std::cmp::Ordering::Less);
        assert_eq!(compare_hnode_configs(&c1, &c3), std::cmp::Ordering::Equal);
        assert_eq!(compare_hnode_configs(&c2, &c1), std::cmp::Ordering::Greater);
    }

    fn pos(x: i32, y: i32) -> u32 {
        pos_to_id(IVec2::new(x, y), 5)
    }

    #[test]
    fn _force_pos_use() {
        let _ = pos(0, 0);
    }
}
