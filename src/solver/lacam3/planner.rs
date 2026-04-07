//! LaCAM3 Planner — main LaCAM* high-level configuration search loop.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/planner.cpp (383 lines)
//!            docs/papers_codes/lacam3/lacam3/include/planner.hpp
//!
//! Wires hnode + lnode + pibt + scatter into the LaCAM* search loop.
//!
//! ## Adaptations to MAFIS
//!
//! - **Arena-based HNode storage**: lacam3 uses raw `HNode*` pointers and an
//!   `unordered_map<Config, HNode*, ConfigHasher>`. We use a `Vec<HNode>`
//!   arena (`hnodes` field) and reference HNodes by `usize` indices
//!   (`HNodeId`). The EXPLORED map becomes `HashMap<Config, HNodeId>`.
//! - **No multi-threading**: lacam3 spawns `PIBT_NUM` threads via `std::async`
//!   to compute candidate configurations in parallel. WASM precludes threading,
//!   so we run a single PIBT sequentially. This may slow search but doesn't
//!   affect solution quality.
//! - **No refiner**: lacam3 has a separate refiner pool that improves the
//!   solution post-hoc via SIPP-based LNS. Skipped in v1 (validation gate
//!   will tell us if it's needed).
//! - **No checkpointing / logging**: instrumentation only.
//! - **Random insert disabled by default**: lacam3 uses `RANDOM_INSERT_PROB1/2`
//!   to occasionally restart from H_init. Kept as a tunable constant.
//!
//! Constants `FLG_*` from lacam3 planner.cpp lines 6-21 are translated into
//! `PlannerConfig` struct fields with the same defaults.

use std::collections::{HashMap, VecDeque};

use rand::Rng;

use crate::core::seed::SeededRng;

use super::dist_table::DistTable;
use super::hnode::{HNode, HNodeId};
use super::instance::{Config, Instance, Solution, is_same_config};
use super::pibt::Pibt;
use super::scatter::Scatter;

/// Static planner configuration (mirrors `Planner::FLG_*` from lacam3).
///
/// REFERENCE: lacam3 planner.cpp lines 6-21.
#[derive(Debug, Clone, Copy)]
pub struct PlannerConfig {
    pub flg_swap: bool,
    pub flg_star: bool,
    pub flg_scatter: bool,
    pub scatter_margin: i32,
    /// Maximum search iterations (analog of lacam3's `Deadline`).
    pub max_iters: usize,
    pub random_insert_prob1: f32,
    pub random_insert_prob2: f32,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            flg_swap: true,
            flg_star: true,
            flg_scatter: true,
            scatter_margin: 10,
            max_iters: 5_000,
            random_insert_prob1: 0.1,
            random_insert_prob2: 0.01,
        }
    }
}

/// LaCAM3 main planner.
///
/// REFERENCE: lacam3 planner.hpp lines 30-109.
pub struct Planner<'a> {
    pub ins: &'a Instance<'a>,
    pub d: &'a DistTable,
    pub config: PlannerConfig,

    pub rng: SeededRng,

    // Arena storage for HNodes (replaces lacam3's raw pointer ownership).
    pub hnodes: Vec<HNode>,

    // Search state.
    pub open: VecDeque<HNodeId>,
    pub explored: HashMap<Config, HNodeId>,
    pub h_init: Option<HNodeId>,
    pub h_goal: Option<HNodeId>,

    pub search_iter: usize,
}

impl<'a> Planner<'a> {
    pub fn new(ins: &'a Instance<'a>, d: &'a DistTable, config: PlannerConfig, seed: u64) -> Self {
        Self {
            ins,
            d,
            config,
            rng: SeededRng::new(seed),
            hnodes: Vec::new(),
            open: VecDeque::new(),
            explored: HashMap::new(),
            h_init: None,
            h_goal: None,
            search_iter: 0,
        }
    }

    /// Heuristic: sum of agent distances to their goals.
    ///
    /// REFERENCE: lacam3 heuristic.cpp lines 5-10.
    pub fn heuristic(&self, q: &Config) -> i32 {
        let mut cost = 0i32;
        for i in 0..self.ins.n {
            cost += self.d.get(i, q[i]);
        }
        cost
    }

    /// Edge cost: count of agents not at their goal at either endpoint.
    ///
    /// REFERENCE: lacam3 planner.cpp `get_edge_cost` lines 275-284.
    pub fn get_edge_cost(&self, c1: &Config, c2: &Config) -> i32 {
        let mut cost = 0i32;
        for i in 0..self.ins.n {
            if c1[i] != self.ins.goals[i] || c2[i] != self.ins.goals[i] {
                cost += 1;
            }
        }
        cost
    }

    /// Create a new high-level node and insert it into EXPLORED.
    ///
    /// REFERENCE: lacam3 planner.cpp `create_highlevel_node` lines 160-168.
    pub fn create_highlevel_node(&mut self, q: Config, parent_id: Option<HNodeId>) -> HNodeId {
        let g_val = match parent_id {
            Some(pid) => {
                let p = &self.hnodes[pid];
                p.g + self.get_edge_cost(&p.c, &q)
            }
            None => 0,
        };
        let h_val = self.heuristic(&q);
        let parent_node = parent_id.map(|pid| &self.hnodes[pid]);
        let h_new = HNode::new(q.clone(), self.d, parent_node, parent_id, g_val, h_val);

        let id = self.hnodes.len();
        self.hnodes.push(h_new);

        // Update parent.neighbor.
        if let Some(pid) = parent_id {
            self.hnodes[pid].neighbor.push(id);
        }
        // self.neighbor.insert(parent) — symmetric link
        // (already implicit via parent pointer; lacam3 uses set for dedup)

        self.explored.insert(q, id);
        id
    }

    /// Backtrack to extract a Solution from a goal HNode.
    ///
    /// REFERENCE: lacam3 planner.cpp `backtrack` lines 196-206.
    pub fn backtrack(&self, h_goal: HNodeId) -> Solution {
        let mut plan: Vec<Config> = Vec::new();
        let mut h: Option<HNodeId> = Some(h_goal);
        while let Some(id) = h {
            plan.push(self.hnodes[id].c.clone());
            h = self.hnodes[id].parent;
        }
        plan.reverse();
        plan
    }

    /// Generate a child configuration via PIBT, respecting the LNode chain.
    ///
    /// REFERENCE: lacam3 planner.cpp `set_new_config` lines 208-248.
    /// Adapted: single-PIBT sequential (no multi-threaded Monte Carlo).
    fn set_new_config(
        &mut self,
        h_id: HNodeId,
        l: &super::lnode::LNode,
        scatter: Option<&Scatter>,
    ) -> Option<Config> {
        let n = self.ins.n;

        // Initialize candidate Q with the LNode constraints.
        let mut q_cand: Config = vec![u32::MAX; n];
        for d_idx in 0..l.depth {
            q_cand[l.who[d_idx] as usize] = l.where_[d_idx];
        }

        // Run PIBT.
        let h_order = self.hnodes[h_id].order.clone();
        let h_c = self.hnodes[h_id].c.clone();
        let mut pibt = Pibt::new(self.ins, self.d, self.rng.rng.random::<u64>(), self.config.flg_swap, scatter);
        if pibt.set_new_config(&h_c, &mut q_cand, &h_order) {
            Some(q_cand)
        } else {
            None
        }
    }

    /// Rewrite paths via Dijkstra after a known-config rediscovery.
    ///
    /// REFERENCE: lacam3 planner.cpp `rewrite` lines 250-273.
    fn rewrite(&mut self, h_from: HNodeId, h_to: HNodeId) {
        // Update neighbor edge.
        if !self.hnodes[h_from].neighbor.contains(&h_to) {
            self.hnodes[h_from].neighbor.push(h_to);
        }

        // Dijkstra propagation.
        let mut q: VecDeque<HNodeId> = VecDeque::new();
        q.push_back(h_from);
        while let Some(n_from) = q.pop_front() {
            let neighbors_snapshot = self.hnodes[n_from].neighbor.clone();
            let nf_g = self.hnodes[n_from].g;
            let nf_c = self.hnodes[n_from].c.clone();
            for n_to in neighbors_snapshot {
                let edge_cost = self.get_edge_cost(&nf_c, &self.hnodes[n_to].c);
                let g_val = nf_g + edge_cost;
                if g_val < self.hnodes[n_to].g {
                    self.hnodes[n_to].g = g_val;
                    let h_val = self.hnodes[n_to].h;
                    self.hnodes[n_to].f = g_val + h_val;
                    self.hnodes[n_to].parent = Some(n_from);
                    q.push_back(n_to);
                    if let Some(h_goal) = self.h_goal {
                        let goal_f = self.hnodes[h_goal].f;
                        if self.hnodes[n_to].f < goal_f {
                            self.open.push_front(n_to);
                        }
                    }
                }
            }
        }
    }

    /// Main LaCAM* search.
    ///
    /// Returns a `Solution` (sequence of Configs) on success, or empty Vec on
    /// timeout/no-solution.
    ///
    /// REFERENCE: lacam3 planner.cpp `Planner::solve` lines 61-158.
    pub fn solve(&mut self) -> Solution {
        // Build optional Scatter heuristic.
        let scatter = if self.config.flg_scatter {
            Some(Scatter::construct(
                self.ins,
                self.d,
                self.rng.rng.random::<u64>(),
                self.config.scatter_margin,
            ))
        } else {
            None
        };
        let scatter_ref = scatter.as_ref();

        // Insert initial HNode.
        // REFERENCE: planner.cpp lines 67-68.
        let h_init = self.create_highlevel_node(self.ins.starts.clone(), None);
        self.h_init = Some(h_init);
        self.open.push_front(h_init);

        // Search loop.
        // REFERENCE: planner.cpp lines 73-145.
        while let Some(&h_id) = self.open.front() {
            self.search_iter += 1;
            if self.search_iter >= self.config.max_iters {
                break;
            }

            // Random restart from H_init or random OPEN node (after initial sol found).
            let h_id = if self.h_goal.is_some()
                && self.rng.rng.random::<f32>() < self.config.random_insert_prob2
            {
                let len = self.open.len();
                let r_idx = self.rng.rng.random_range(0..len);
                self.open[r_idx]
            } else {
                h_id
            };

            // Lower bound check.
            if let Some(h_goal) = self.h_goal {
                if self.hnodes[h_id].f >= self.hnodes[h_goal].f {
                    self.open.pop_front();
                    continue;
                }
            }

            // Goal check.
            // REFERENCE: planner.cpp lines 105-114.
            if self.h_goal.is_none() && is_same_config(&self.hnodes[h_id].c, &self.ins.goals) {
                self.h_goal = Some(h_id);
                if !self.config.flg_star {
                    break;
                }
                continue;
            }

            // Low-level search: get next LNode constraint from this HNode.
            // REFERENCE: planner.cpp lines 117-121.
            let l_opt = self.hnodes[h_id].get_next_lowlevel_node(self.ins.grid, &mut self.rng);
            let l = match l_opt {
                Some(l) => l,
                None => {
                    self.open.pop_front();
                    continue;
                }
            };

            // Generate successor config via PIBT.
            // REFERENCE: planner.cpp lines 124-127.
            let q_to_opt = self.set_new_config(h_id, &l, scatter_ref);
            let q_to = match q_to_opt {
                Some(q) => q,
                None => continue,
            };

            // Check explored.
            // REFERENCE: planner.cpp lines 129-144.
            if let Some(&existing) = self.explored.get(&q_to) {
                self.rewrite(h_id, existing);
                if self.rng.rng.random::<f32>() >= self.config.random_insert_prob1 {
                    self.open.push_front(existing);
                } else {
                    self.open.push_front(self.h_init.unwrap());
                }
            } else {
                let h_new = self.create_highlevel_node(q_to, Some(h_id));
                self.open.push_front(h_new);
            }
        }

        // Extract solution.
        if let Some(h_goal) = self.h_goal {
            self.backtrack(h_goal)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use bevy::prelude::*;

    #[test]
    fn planner_solves_trivial_one_agent() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let starts = vec![IVec2::new(0, 0)];
        let goals = vec![IVec2::new(4, 0)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);

        let mut planner = Planner::new(&ins, &dt, PlannerConfig::default(), 42);
        let solution = planner.solve();

        assert!(!solution.is_empty(), "should find a solution");
        // First config = start, last = goal
        let first = &solution[0];
        let last = solution.last().unwrap();
        assert_eq!(first[0], super::super::instance::pos_to_id(IVec2::new(0, 0), 5));
        assert_eq!(last[0], super::super::instance::pos_to_id(IVec2::new(4, 0), 5));
    }

    #[test]
    fn planner_solves_two_agents_no_collision() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(7, 7)));
        let starts = vec![IVec2::new(0, 3), IVec2::new(6, 3)];
        let goals = vec![IVec2::new(6, 3), IVec2::new(0, 3)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);

        let mut planner = Planner::new(&ins, &dt, PlannerConfig::default(), 42);
        let solution = planner.solve();

        assert!(!solution.is_empty(), "should find solution for two crossing agents");
        // Verify no vertex collisions in any timestep
        for (t, config) in solution.iter().enumerate() {
            assert_ne!(
                config[0], config[1],
                "vertex collision at t={t}: {config:?}"
            );
        }
        // Verify final positions match goals
        let last = solution.last().unwrap();
        assert_eq!(last[0], super::super::instance::pos_to_id(IVec2::new(6, 3), 7));
        assert_eq!(last[1], super::super::instance::pos_to_id(IVec2::new(0, 3), 7));
    }
}
