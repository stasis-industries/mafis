//! CollisionTable — fast collision tracking for SUO/scatter.
//!
//! REFERENCE: docs/papers_codes//lacam3/src/collision_table.cpp (96 lines)
//!            docs/papers_codes/lacam3/lacam3/include/collision_table.hpp
//!
//! Tracks per-cell, per-timestep agent occupancy for the scatter heuristic.
//! Used by `Scatter::construct` to compute collision counts when reordering
//! single-agent paths in the SUO loop.
//!
//! ## Adaptations
//!
//! lacam3 stores `body: vector<vector<vector<int>>>` (cell × time × agent list)
//! and `body_last: vector<vector<int>>` (cell × list of agent end-times).
//! We mirror this structure with `Vec<Vec<Vec<u32>>>` for `body`. The lazy
//! resizing logic (`while body[v->id].size() <= t`) is preserved.

use super::instance::{Instance, Path};

/// Per-cell, per-timestep agent occupancy table.
///
/// REFERENCE: lacam3 collision_table.hpp lines 10-25.
pub struct CollisionTable {
    /// `body[cell_id][timestep]` = list of agents at that cell at that time.
    pub body: Vec<Vec<Vec<u32>>>,
    /// `body_last[cell_id]` = list of "last timesteps" (T_i) for agents whose
    /// path ends at that cell. Used for goal collision counting.
    pub body_last: Vec<Vec<i32>>,
    /// Total number of currently-counted collisions.
    pub collision_cnt: i32,
    pub n: u32,
}

impl CollisionTable {
    pub fn new(ins: &Instance) -> Self {
        let v_size = ins.v_size;
        Self {
            body: vec![Vec::new(); v_size],
            body_last: vec![Vec::new(); v_size],
            collision_cnt: 0,
            n: ins.n as u32,
        }
    }

    /// Compute the collision cost incurred by moving from `v_from` to `v_to`
    /// at timestep `t_from → t_from+1`.
    ///
    /// REFERENCE: lacam3 collision_table.cpp `getCollisionCost` lines 13-35.
    pub fn get_collision_cost(&self, v_from: u32, v_to: u32, t_from: usize) -> i32 {
        let t_to = t_from + 1;
        let mut collision = 0i32;

        // Vertex collision: someone else at v_to at t_to.
        if t_to < self.body[v_to as usize].len() {
            collision += self.body[v_to as usize][t_to].len() as i32;
        }

        // Edge collision (swap): someone going v_to → v_from at the same step.
        if t_to < self.body[v_from as usize].len() && t_from < self.body[v_to as usize].len() {
            for &j in &self.body[v_from as usize][t_to] {
                for &k in &self.body[v_to as usize][t_from] {
                    if j == k {
                        collision += 1;
                    }
                }
            }
        }

        // Goal collision: another agent has parked at v_to and we'd arrive
        // after their last timestep, intersecting their endpoint reservation.
        for &last_timestep in &self.body_last[v_to as usize] {
            if (t_to as i32) > last_timestep {
                collision += 1;
            }
        }

        collision
    }

    /// Add agent `i`'s path into the table, updating collision count.
    ///
    /// REFERENCE: lacam3 collision_table.cpp `enrollPath` lines 37-58.
    pub fn enroll_path(&mut self, i: u32, path: &Path) {
        if path.is_empty() {
            return;
        }
        let t_i = path.len() - 1;

        for t in 0..=t_i {
            let v = path[t];
            // Update collision count BEFORE registering this step.
            if t > 0 {
                self.collision_cnt += self.get_collision_cost(path[t - 1], path[t], t - 1);
            }
            // Register: lazy resize body[v] up to t.
            while self.body[v as usize].len() <= t {
                self.body[v as usize].push(Vec::new());
            }
            self.body[v as usize][t].push(i);
        }

        // Goal handling: register endpoint and add collisions for any agents
        // already passing through this cell after T_i.
        let goal = path[t_i];
        self.body_last[goal as usize].push(t_i as i32);
        let entry_len = self.body[goal as usize].len();
        for t in (t_i + 1)..entry_len {
            self.collision_cnt += self.body[goal as usize][t].len() as i32;
        }
    }

    /// Remove agent `i`'s path from the table.
    ///
    /// REFERENCE: lacam3 collision_table.cpp `clearPath` lines 60-96.
    pub fn clear_path(&mut self, i: u32, path: &Path) {
        if path.is_empty() {
            return;
        }
        let t_i = path.len() - 1;

        for t in 0..=t_i {
            let v = path[t];
            // Remove i from body[v][t]
            if let Some(pos) =
                self.body[v as usize].get(t).and_then(|entry| entry.iter().position(|&a| a == i))
            {
                self.body[v as usize][t].remove(pos);
            }
            // Update collision count AFTER removal.
            if t > 0 {
                self.collision_cnt -= self.get_collision_cost(path[t - 1], path[t], t - 1);
            }
        }

        // Goal handling: remove from body_last and decrement collision count.
        let goal = path[t_i];
        if let Some(pos) = self.body_last[goal as usize].iter().position(|&last| last == t_i as i32)
        {
            self.body_last[goal as usize].remove(pos);
        }
        let entry_len = self.body[goal as usize].len();
        for t in (t_i + 1)..entry_len {
            self.collision_cnt -= self.body[goal as usize][t].len() as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::instance::Instance;
    use super::*;
    use crate::core::grid::GridMap;
    use bevy::prelude::*;

    fn make_ct() -> CollisionTable {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let ins = Instance::new(grid, vec![IVec2::new(0, 0); 2], vec![IVec2::new(4, 4); 2]);
        CollisionTable::new(&ins)
    }

    #[test]
    fn ct_empty_no_collisions() {
        let ct = make_ct();
        assert_eq!(ct.collision_cnt, 0);
    }

    #[test]
    fn ct_enroll_then_clear_is_symmetric() {
        let mut ct = make_ct();
        let path: Path = vec![0, 1, 2, 3, 4]; // straight line
        ct.enroll_path(0, &path);
        let cnt_after_enroll = ct.collision_cnt;
        ct.clear_path(0, &path);
        assert_eq!(ct.collision_cnt, 0);
        // First enrollment of an isolated path = 0 collisions
        assert_eq!(cnt_after_enroll, 0);
    }

    #[test]
    fn ct_two_paths_at_same_cell_same_time_collide() {
        let mut ct = make_ct();
        let path1: Path = vec![0, 1, 2];
        let path2: Path = vec![5, 1, 7]; // both at cell 1 at t=1
        ct.enroll_path(0, &path1);
        let before = ct.collision_cnt;
        ct.enroll_path(1, &path2);
        let after = ct.collision_cnt;
        // Vertex collision at (cell=1, t=1): expect collision_cnt to increase
        assert!(after > before, "expected collision detected at cell 1 t=1");
    }
}
