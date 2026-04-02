//! Common types shared between Token Passing solvers.
//!
//! Extracted so that `token_passing.rs` and future solvers (e.g. TPTS) can
//! reuse `Token`, `MasterConstraintIndex`, and `dir_ordinal` without
//! duplicating the logic.

use bevy::prelude::*;
use std::collections::VecDeque;

use crate::solver::shared::astar::ConstraintChecker;

// ---------------------------------------------------------------------------
// TOKEN — shared path store
// ---------------------------------------------------------------------------

/// The TOKEN stores all agents' planned paths as position sequences.
pub(super) struct Token {
    pub(super) paths: Vec<VecDeque<IVec2>>,
}

impl Token {
    pub(super) fn new() -> Self {
        Self { paths: Vec::new() }
    }

    pub(super) fn reset(&mut self, n: usize) {
        self.paths.clear();
        self.paths.resize(n, VecDeque::new());
    }

    pub(super) fn advance(&mut self) {
        for path in &mut self.paths {
            if !path.is_empty() {
                path.pop_front();
            }
        }
    }

    pub(super) fn set_path(&mut self, agent: usize, positions: Vec<IVec2>) {
        if agent < self.paths.len() {
            self.paths[agent] = positions.into();
        }
    }
}

// ---------------------------------------------------------------------------
// MasterConstraintIndex — reference-counted constraints for O(1) add/remove
// ---------------------------------------------------------------------------

/// Reference-counted constraint index. Instead of rebuilding from scratch for
/// each agent, maintains counts of agents per (pos, time) and (edge, time).
/// When planning for agent i: remove agent i's path, plan, add new path back.
pub(super) struct MasterConstraintIndex {
    vertex_counts: Vec<u8>,
    edge_counts: Vec<u8>,
    width: i32,
    stride: usize,
    cells: usize,
}

impl MasterConstraintIndex {
    pub(super) fn new() -> Self {
        Self { vertex_counts: Vec::new(), edge_counts: Vec::new(), width: 0, stride: 0, cells: 0 }
    }

    pub(super) fn reset(&mut self, width: i32, height: i32, max_time: u64) {
        let stride = (max_time + 1) as usize;
        let cells = (width * height) as usize;
        let vtotal = cells * stride;
        let etotal = cells * 5 * stride;

        if self.vertex_counts.len() != vtotal || self.width != width {
            self.width = width;
            self.stride = stride;
            self.cells = cells;
            self.vertex_counts = vec![0u8; vtotal];
            self.edge_counts = vec![0u8; etotal];
        } else {
            self.vertex_counts.fill(0);
            self.edge_counts.fill(0);
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

    /// Add an agent's path to the master index (increment reference counts).
    pub(super) fn add_path(&mut self, path: &VecDeque<IVec2>, max_time: u64) {
        if path.is_empty() {
            return;
        }

        for (t, &pos) in path.iter().enumerate() {
            let time = t as u64;
            if time > max_time {
                break;
            }
            let idx = self.vertex_idx(pos, time);
            if idx < self.vertex_counts.len() {
                self.vertex_counts[idx] = self.vertex_counts[idx].saturating_add(1);
            }

            // Edge constraint: prevent swapping
            if t > 0 {
                let prev = path[t - 1];
                if prev != pos {
                    let eidx = self.edge_idx(pos, prev, (t - 1) as u64);
                    if eidx < self.edge_counts.len() {
                        self.edge_counts[eidx] = self.edge_counts[eidx].saturating_add(1);
                    }
                }
            }
        }

        // After path ends, agent stays at last position
        if let Some(&last) = path.back() {
            let path_end = path.len() as u64;
            for t in path_end..=max_time {
                let idx = self.vertex_idx(last, t);
                if idx < self.vertex_counts.len() {
                    self.vertex_counts[idx] = self.vertex_counts[idx].saturating_add(1);
                }
            }
        }
    }

    /// Remove an agent's path from the master index (decrement reference counts).
    pub(super) fn remove_path(&mut self, path: &VecDeque<IVec2>, max_time: u64) {
        if path.is_empty() {
            return;
        }

        for (t, &pos) in path.iter().enumerate() {
            let time = t as u64;
            if time > max_time {
                break;
            }
            let idx = self.vertex_idx(pos, time);
            if idx < self.vertex_counts.len() {
                self.vertex_counts[idx] = self.vertex_counts[idx].saturating_sub(1);
            }

            if t > 0 {
                let prev = path[t - 1];
                if prev != pos {
                    let eidx = self.edge_idx(pos, prev, (t - 1) as u64);
                    if eidx < self.edge_counts.len() {
                        self.edge_counts[eidx] = self.edge_counts[eidx].saturating_sub(1);
                    }
                }
            }
        }

        if let Some(&last) = path.back() {
            let path_end = path.len() as u64;
            for t in path_end..=max_time {
                let idx = self.vertex_idx(last, t);
                if idx < self.vertex_counts.len() {
                    self.vertex_counts[idx] = self.vertex_counts[idx].saturating_sub(1);
                }
            }
        }
    }
}

/// Map delta to direction ordinal: N=0, S=1, E=2, W=3, Self=4
#[inline]
pub(super) fn dir_ordinal(from: IVec2, to: IVec2) -> usize {
    let d = to - from;
    match (d.x, d.y) {
        (0, 1) => 0,
        (0, -1) => 1,
        (1, 0) => 2,
        (-1, 0) => 3,
        _ => 4,
    }
}

impl ConstraintChecker for MasterConstraintIndex {
    #[inline]
    fn is_vertex_blocked(&self, pos: IVec2, time: u64) -> bool {
        let idx = self.vertex_idx(pos, time);
        idx < self.vertex_counts.len() && self.vertex_counts[idx] > 0
    }

    #[inline]
    fn is_edge_blocked(&self, from: IVec2, to: IVec2, time: u64) -> bool {
        let idx = self.edge_idx(from, to, time);
        idx < self.edge_counts.len() && self.edge_counts[idx] > 0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_reset_and_advance() {
        let mut token = Token::new();
        token.reset(3);
        assert_eq!(token.paths.len(), 3);
        token.set_path(0, vec![IVec2::new(0, 0), IVec2::new(1, 0), IVec2::new(2, 0)]);
        token.advance();
        assert_eq!(token.paths[0].front(), Some(&IVec2::new(1, 0)));
        assert_eq!(token.paths[0].len(), 2);
    }

    #[test]
    fn master_ci_add_remove_symmetric() {
        let mut mci = MasterConstraintIndex::new();
        mci.reset(5, 5, 10);
        let path: VecDeque<IVec2> =
            vec![IVec2::new(0, 0), IVec2::new(1, 0), IVec2::new(2, 0)].into();
        mci.add_path(&path, 10);
        assert!(mci.is_vertex_blocked(IVec2::new(1, 0), 1));
        assert!(mci.is_vertex_blocked(IVec2::new(2, 0), 5));
        mci.remove_path(&path, 10);
        assert!(!mci.is_vertex_blocked(IVec2::new(1, 0), 1));
        assert!(!mci.is_vertex_blocked(IVec2::new(2, 0), 5));
    }

    #[test]
    fn dir_ordinal_values() {
        let o = IVec2::ZERO;
        assert_eq!(dir_ordinal(o, IVec2::new(0, 1)), 0);
        assert_eq!(dir_ordinal(o, IVec2::new(0, -1)), 1);
        assert_eq!(dir_ordinal(o, IVec2::new(1, 0)), 2);
        assert_eq!(dir_ordinal(o, IVec2::new(-1, 0)), 3);
        assert_eq!(dir_ordinal(o, o), 4);
    }
}
