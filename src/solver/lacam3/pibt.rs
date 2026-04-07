//! LaCAM3 PIBT — specialized PIBT for use as configuration generator.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/pibt.cpp (230 lines)
//!            docs/papers_codes/lacam3/lacam3/include/pibt.hpp
//! Paper: Okumura — "Priority Inheritance with Backtracking for Iterative
//! Multi-agent Path Finding", AIJ 2022.
//!
//! ## Why this is NOT shared with `src/solver/shared/pibt_core.rs`
//!
//! lacam3's PIBT has been engineered specifically for use as a configuration
//! generator inside the LaCAM* high-level search. Specifically:
//! - It accepts a partial assignment from the LNode constraint chain
//! - It integrates with the Scatter (SUO) heuristic via `prioritized_vertex`
//! - It implements the swap technique (`is_swap_required`, `is_swap_possible`)
//!   from the LaCAM* paper
//! - The recursive `funcPIBT` accesses agent-priority `order` from the HNode
//!
//! Sharing code with `pibt_core.rs` would force one of the two implementations
//! to drift, undermining fidelity to either pibt2 (the standalone PIBT
//! reference) or lacam3 (the engineered variant).

use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use rand::Rng;
#[allow(unused_imports)]
use rand::random;

use super::dist_table::DistTable;
use super::instance::{Config, Instance};
use super::scatter::Scatter;

/// Sentinel value for "no agent occupies this cell". Set to N (the agent count).
///
/// REFERENCE: lacam3 pibt.hpp line 26 `const int NO_AGENT`.
const NO_AGENT_OFFSET: u32 = 0;

/// LaCAM3 PIBT configuration generator.
///
/// REFERENCE: lacam3 pibt.hpp lines 16-50.
pub struct Pibt<'a> {
    pub n: usize,
    pub v_size: usize,

    /// "no agent" sentinel = N (set in constructor)
    pub no_agent: u32,

    /// occupied_now[v_id] = agent occupying cell v at the current step (or NO_AGENT)
    pub occupied_now: Vec<u32>,
    /// occupied_next[v_id] = agent reserved to enter cell v at next step (or NO_AGENT)
    pub occupied_next: Vec<u32>,

    /// Per-cell random tie-breaker (refreshed each call to set_new_config).
    /// REFERENCE: lacam3 pibt.hpp line 30 `vector<float> tie_breakers`.
    pub tie_breakers: Vec<f32>,

    /// Whether to use the swap technique (LaCAM* paper).
    /// REFERENCE: lacam3 pibt.hpp line 33 `bool flg_swap`.
    pub flg_swap: bool,

    /// Optional Scatter heuristic for prioritized vertex selection.
    /// REFERENCE: lacam3 pibt.hpp line 36 `Scatter *scatter`.
    pub scatter: Option<&'a Scatter>,

    /// Reference to grid for neighbor queries.
    pub grid: &'a GridMap,
    /// Reference to instance for goal lookups.
    pub ins: &'a Instance<'a>,
    /// Reference to distance table.
    pub d: &'a DistTable,
    /// RNG for random tie-breakers and shuffles.
    pub rng: SeededRng,
}

impl<'a> Pibt<'a> {
    pub fn new(
        ins: &'a Instance<'a>,
        d: &'a DistTable,
        seed: u64,
        flg_swap: bool,
        scatter: Option<&'a Scatter>,
    ) -> Self {
        let n = ins.n;
        let v_size = ins.v_size;
        let no_agent = n as u32 + NO_AGENT_OFFSET;
        Self {
            n,
            v_size,
            no_agent,
            occupied_now: vec![no_agent; v_size],
            occupied_next: vec![no_agent; v_size],
            tie_breakers: vec![0.0; v_size],
            flg_swap,
            scatter,
            grid: ins.grid,
            ins,
            d,
            rng: SeededRng::new(seed),
        }
    }

    /// Generate a new configuration `q_to` from `q_from`, respecting any
    /// pre-set entries in `q_to` (the partial assignment from the LNode chain).
    ///
    /// Returns `true` if all agents were assigned successfully.
    ///
    /// REFERENCE: lacam3 pibt.cpp `set_new_config` lines 22-64.
    pub fn set_new_config(&mut self, q_from: &Config, q_to: &mut Config, order: &[u32]) -> bool {
        let mut success = true;

        // Setup cache & constraint check.
        // REFERENCE: lines 27-46.
        for i in 0..self.n {
            self.occupied_now[q_from[i] as usize] = i as u32;
            if q_to[i] != self.no_agent && q_to[i] != u32::MAX {
                // (constraint already set from LNode chain)
                let to = q_to[i];
                // Vertex collision
                if self.occupied_next[to as usize] != self.no_agent {
                    success = false;
                    break;
                }
                // Swap collision
                let j = self.occupied_now[to as usize];
                if j != self.no_agent && j as usize != i && q_to[j as usize] == q_from[i] {
                    success = false;
                    break;
                }
                self.occupied_next[to as usize] = i as u32;
            }
        }

        if success {
            // Plan unassigned agents in order via funcPIBT.
            // REFERENCE: lines 48-55.
            for &i in order {
                let i = i as usize;
                if q_to[i] == u32::MAX && !self.func_pibt(i, q_from, q_to) {
                    success = false;
                    break;
                }
            }
        }

        // Cleanup: reset occupied_now/next.
        // REFERENCE: lines 57-61.
        for i in 0..self.n {
            self.occupied_now[q_from[i] as usize] = self.no_agent;
            if q_to[i] != u32::MAX {
                self.occupied_next[q_to[i] as usize] = self.no_agent;
            }
        }

        success
    }

    /// Recursive PIBT step for agent `i`.
    ///
    /// REFERENCE: lacam3 pibt.cpp `funcPIBT` lines 66-147.
    fn func_pibt(&mut self, i: usize, q_from: &Config, q_to: &mut Config) -> bool {
        // Build candidate set: neighbors + self.
        // REFERENCE: lines 80-85.
        let from_v = q_from[i];
        let raw_neighbors = super::instance::neighbors(self.grid, from_v);
        let k = raw_neighbors.len();

        // Get prioritized vertex from scatter (if available).
        // REFERENCE: lines 70-77.
        let prioritized_vertex: Option<u32> = self.scatter.and_then(|s| s.get_next(i, from_v));

        // C_next[i] candidates (neighbors + self), sized K+1.
        let mut c_next: smallvec::SmallVec<[u32; 5]> = smallvec::SmallVec::new();
        for &u in raw_neighbors.iter() {
            c_next.push(u);
            self.tie_breakers[u as usize] = self.rng.rng.random::<f32>();
        }
        c_next.push(from_v);

        // Sort by (D->get + tie_breaker), with prioritized_vertex always first.
        // REFERENCE: lines 88-94.
        let d = self.d;
        let tb = &self.tie_breakers;
        let pv = prioritized_vertex;
        c_next.sort_by(|&v, &u| {
            if Some(v) == pv {
                return std::cmp::Ordering::Less;
            }
            if Some(u) == pv {
                return std::cmp::Ordering::Greater;
            }
            let fv = d.get(i, v) as f32 + tb[v as usize];
            let fu = d.get(i, u) as f32 + tb[u as usize];
            fv.partial_cmp(&fu).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Emulate swap (LaCAM* technique).
        // REFERENCE: lines 96-104.
        let mut swap_agent = self.no_agent;
        if self.flg_swap {
            swap_agent = self.is_swap_required_and_possible(i, q_from, q_to, &c_next);
            if swap_agent != self.no_agent {
                // Reverse the candidate ordering.
                c_next[..k + 1].reverse();
            }
        }

        // Main loop: try each candidate in order.
        // REFERENCE: lines 117-141.
        for k_idx in 0..k + 1 {
            let u = c_next[k_idx];

            // Avoid vertex conflicts.
            if self.occupied_next[u as usize] != self.no_agent {
                continue;
            }

            let j = self.occupied_now[u as usize];

            // Avoid swap conflicts with constraints.
            if j != self.no_agent && q_to[j as usize] == from_v {
                continue;
            }

            // Reserve.
            self.occupied_next[u as usize] = i as u32;
            q_to[i] = u;

            // Priority inheritance.
            // REFERENCE: lines 134-136.
            if j != self.no_agent && u != from_v && q_to[j as usize] == u32::MAX {
                let j_usize = j as usize;
                if !self.func_pibt(j_usize, q_from, q_to) {
                    continue;
                }
            }

            // Success: optionally apply swap operation.
            if self.flg_swap && k_idx == 0 && swap_agent != self.no_agent {
                let sa = swap_agent as usize;
                if q_to[sa] == u32::MAX && self.occupied_next[from_v as usize] == self.no_agent {
                    self.occupied_next[from_v as usize] = swap_agent;
                    q_to[sa] = from_v;
                }
            }
            return true;
        }

        // Failed: stay at from_v.
        // REFERENCE: lines 143-146.
        self.occupied_next[from_v as usize] = i as u32;
        q_to[i] = from_v;
        false
    }

    /// Check if a swap is required and possible for agent `i`.
    ///
    /// REFERENCE: lacam3 pibt.cpp `is_swap_required_and_possible` lines 149-176.
    fn is_swap_required_and_possible(
        &self,
        i: usize,
        q_from: &Config,
        q_to: &Config,
        c_next: &[u32],
    ) -> u32 {
        // Agent j occupying our preferred next cell.
        let preferred = c_next[0];
        let j = self.occupied_now[preferred as usize];
        if j != self.no_agent
            && j as usize != i
            && q_to[j as usize] == u32::MAX
            && self.is_swap_required(i, j as usize, q_from[i], q_from[j as usize])
            && self.is_swap_possible(q_from[j as usize], q_from[i])
        {
            return j;
        }

        // Push & swap clear operation.
        // REFERENCE: lines 162-174.
        if preferred != q_from[i] {
            for u in super::instance::neighbors(self.grid, q_from[i]) {
                let k = self.occupied_now[u as usize];
                if k != self.no_agent
                    && preferred != q_from[k as usize]
                    && self.is_swap_required(k as usize, i, q_from[i], preferred)
                    && self.is_swap_possible(preferred, q_from[i])
                {
                    return k;
                }
            }
        }
        self.no_agent
    }

    /// Check whether a swap is required from `pusher`'s perspective.
    ///
    /// REFERENCE: lacam3 pibt.cpp `is_swap_required` lines 178-205.
    fn is_swap_required(
        &self,
        pusher: usize,
        puller: usize,
        v_pusher_origin: u32,
        v_puller_origin: u32,
    ) -> bool {
        let mut v_pusher = v_pusher_origin;
        let mut v_puller = v_puller_origin;
        loop {
            if !(self.d.get(pusher, v_puller) < self.d.get(pusher, v_pusher)) {
                break;
            }
            let neighbors = super::instance::neighbors(self.grid, v_puller);
            let mut n = neighbors.len();
            let mut tmp: u32 = u32::MAX;
            for u in neighbors {
                let i_at = self.occupied_now[u as usize];
                let is_blocked_singleton = {
                    let n_count = super::instance::neighbors(self.grid, u).len();
                    n_count == 1
                        && i_at != self.no_agent
                        && self.ins.goals[i_at as usize] == u
                };
                if u == v_pusher || is_blocked_singleton {
                    n -= 1;
                } else {
                    tmp = u;
                }
            }
            if n >= 2 {
                return false; // can swap at v_l
            }
            if n == 0 {
                break;
            }
            v_pusher = v_puller;
            v_puller = tmp;
        }

        let cond_a = self.d.get(puller, v_pusher) < self.d.get(puller, v_puller);
        let cond_b = self.d.get(pusher, v_pusher) == 0
            || self.d.get(pusher, v_puller) < self.d.get(pusher, v_pusher);
        cond_a && cond_b
    }

    /// Check whether a swap is geometrically possible.
    ///
    /// REFERENCE: lacam3 pibt.cpp `is_swap_possible` lines 207-230.
    fn is_swap_possible(&self, v_pusher_origin: u32, v_puller_origin: u32) -> bool {
        let mut v_pusher = v_pusher_origin;
        let mut v_puller = v_puller_origin;
        loop {
            if v_puller == v_pusher_origin {
                return false; // avoid loop
            }
            let neighbors = super::instance::neighbors(self.grid, v_puller);
            let mut n = neighbors.len();
            let mut tmp: u32 = u32::MAX;
            for u in neighbors {
                let i_at = self.occupied_now[u as usize];
                let is_blocked_singleton = {
                    let n_count = super::instance::neighbors(self.grid, u).len();
                    n_count == 1
                        && i_at != self.no_agent
                        && self.ins.goals[i_at as usize] == u
                };
                if u == v_pusher || is_blocked_singleton {
                    n -= 1;
                } else {
                    tmp = u;
                }
            }
            if n >= 2 {
                return true;
            }
            if n == 0 {
                return false;
            }
            v_pusher = v_puller;
            v_puller = tmp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn pibt_constructs() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let starts = vec![IVec2::new(0, 0), IVec2::new(4, 4)];
        let goals = vec![IVec2::new(4, 4), IVec2::new(0, 0)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);
        let pibt = Pibt::new(&ins, &dt, 42, true, None);
        assert_eq!(pibt.n, 2);
        assert_eq!(pibt.v_size, 25);
    }

    #[test]
    fn pibt_two_agents_no_collision_one_step() {
        let grid: &'static GridMap = Box::leak(Box::new(GridMap::new(5, 5)));
        let starts = vec![IVec2::new(0, 0), IVec2::new(4, 4)];
        let goals = vec![IVec2::new(4, 4), IVec2::new(0, 0)];
        let ins = Instance::new(grid, starts, goals);
        let dt = DistTable::new(grid, &ins);
        let mut pibt = Pibt::new(&ins, &dt, 42, true, None);

        let q_from = ins.starts.clone();
        let mut q_to = vec![u32::MAX; ins.n];
        let order: Vec<u32> = (0..ins.n as u32).collect();
        let success = pibt.set_new_config(&q_from, &mut q_to, &order);
        assert!(success);
        // Both agents should have moved (or stayed)
        assert_ne!(q_to[0], u32::MAX);
        assert_ne!(q_to[1], u32::MAX);
        // No vertex collision
        assert_ne!(q_to[0], q_to[1]);
    }
}
