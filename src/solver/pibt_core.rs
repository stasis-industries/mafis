//! PibtCore — extracted PIBT algorithm shared by standalone PIBT, PIBT-Window
//! planner, and RHCR fallback.
//!
//! Contains the core one-step priority-inheritance logic without any solver
//! trait wrappers. All consumers compose this instead of duplicating it.

use bevy::prelude::*;

use crate::core::action::{Action, Direction};
use crate::core::grid::GridMap;

use super::heuristics::{DistanceMap, delta_to_action};

// ---------------------------------------------------------------------------
// Grid-indexed occupation map (replaces HashMap for O(1) with no hashing)
// ---------------------------------------------------------------------------

/// Sentinel value meaning "no agent occupies this cell".
const NO_AGENT: usize = usize::MAX;

/// Flat grid-indexed occupation buffer. Lookup is a single array index.
/// Uses lazy clearing: only cells written since the last `reset()` are
/// cleared, avoiding a full memset on large grids every tick.
struct OccGrid {
    buf: Vec<usize>,   // grid_w * grid_h, values are agent index or NO_AGENT
    dirty: Vec<usize>, // indices written since last reset
    w: i32,
}

impl OccGrid {
    fn new() -> Self {
        Self { buf: Vec::new(), dirty: Vec::new(), w: 0 }
    }

    /// Prepare for a new step. On first call (or grid resize) allocates
    /// and fills the buffer. On subsequent calls only clears dirty cells.
    fn reset(&mut self, grid_w: i32, grid_h: i32) {
        let size = (grid_w * grid_h) as usize;
        if self.buf.len() != size {
            // Grid dimensions changed — full realloc
            self.w = grid_w;
            self.buf.clear();
            self.buf.resize(size, NO_AGENT);
            self.dirty.clear();
        } else {
            // Same size — lazy clear only the cells we touched
            self.w = grid_w;
            for &i in &self.dirty {
                self.buf[i] = NO_AGENT;
            }
            self.dirty.clear();
        }
    }

    #[inline]
    fn idx(&self, pos: IVec2) -> usize {
        (pos.y * self.w + pos.x) as usize
    }

    #[inline]
    fn get(&self, pos: IVec2) -> Option<usize> {
        let i = self.idx(pos);
        let v = *self.buf.get(i)?;
        if v == NO_AGENT { None } else { Some(v) }
    }

    #[inline]
    fn set(&mut self, pos: IVec2, agent: usize) {
        let i = self.idx(pos);
        if i < self.buf.len() {
            self.buf[i] = agent;
            self.dirty.push(i);
        }
    }

    #[inline]
    fn remove_if_eq(&mut self, pos: IVec2, agent: usize) {
        let i = self.idx(pos);
        if i < self.buf.len() && self.buf[i] == agent {
            self.buf[i] = NO_AGENT;
            // No need to push to dirty — cell is already NO_AGENT
        }
    }
}

// ---------------------------------------------------------------------------
// PibtCore
// ---------------------------------------------------------------------------

pub struct PibtCore {
    /// Priority per agent — accumulated across steps.
    priorities: Vec<f32>,
    /// Reusable scratch buffers (avoid per-step allocation).
    next_pos_buf: Vec<IVec2>,
    decided_buf: Vec<bool>,
    current_occ: OccGrid,
    next_occ: OccGrid,
    order_buf: Vec<usize>,
    actions_buf: Vec<Action>,
    /// Shuffle seed — incremented each step for deterministic randomization.
    /// Used to shuffle equal-distance candidates (matches reference C++ behavior).
    shuffle_seed: u64,
}

impl Default for PibtCore {
    fn default() -> Self {
        Self::new()
    }
}

impl PibtCore {
    pub fn new() -> Self {
        Self {
            priorities: Vec::new(),
            next_pos_buf: Vec::new(),
            decided_buf: Vec::new(),
            current_occ: OccGrid::new(),
            next_occ: OccGrid::new(),
            order_buf: Vec::new(),
            actions_buf: Vec::new(),
            shuffle_seed: 0,
        }
    }

    pub fn reset(&mut self) {
        self.priorities.clear();
        self.shuffle_seed = 0;
    }

    /// Set shuffle seed from the simulation tick. This ensures deterministic
    /// tie-breaking after rewind (the seed matches the original run at the
    /// same tick, regardless of solver reset).
    pub fn set_shuffle_seed(&mut self, tick: u64) {
        self.shuffle_seed = tick;
    }

    /// Get a copy of the current priorities for snapshot saving.
    pub fn priorities(&self) -> &[f32] {
        &self.priorities
    }

    /// Restore priorities from a snapshot. The length must match the agent
    /// count on the next step, or they'll be reinitialized (harmless fallback).
    pub fn set_priorities(&mut self, priorities: &[f32]) {
        self.priorities.clear();
        self.priorities.extend_from_slice(priorities);
    }

    /// Run one PIBT step for all agents. Returns one action per agent.
    ///
    /// `positions` and `goals` must be the same length.
    /// `dist_maps` must be aligned with agents (one per agent).
    pub fn one_step(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
    ) -> &[Action] {
        self.one_step_impl(positions, goals, grid, dist_maps, &[], &[], None)
    }

    /// Run one PIBT step with task-awareness (reference PIBT_MAPD behavior).
    ///
    /// `has_task[i]` = true means agent i has an active task and should be
    /// prioritized over idle agents. Matches the reference C++ PIBT_MAPD
    /// where assigned agents outprioritize free agents.
    pub fn one_step_with_tasks(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
        has_task: &[bool],
    ) -> &[Action] {
        self.one_step_impl(positions, goals, grid, dist_maps, &[], has_task, None)
    }

    /// Run one PIBT step with task-awareness and a cell-level guidance bias.
    ///
    /// `bias_fn(pos, agent_index) -> f64` is added to the raw distance when
    /// sorting candidates. Lower total = preferred. Pass `None` for normal behavior.
    pub fn one_step_with_bias(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
        has_task: &[bool],
        bias_fn: &dyn Fn(IVec2, usize) -> f64,
    ) -> &[Action] {
        self.one_step_impl(positions, goals, grid, dist_maps, &[], has_task, Some(bias_fn))
    }

    /// Run one PIBT step with sequential APF (paper-accurate PIBT+APF).
    ///
    /// After each agent commits its next position, its future path is projected
    /// and an APF field is added to `apf_field`. Subsequent agents see the
    /// accumulated field when sorting candidates.
    ///
    /// Reference: Pertzovsky et al., arXiv:2505.22753v1, Equations 11-12.
    pub fn one_step_with_apf(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
        has_task: &[bool],
        apf_field: &mut Vec<f64>,
        apf_w: f64,
        apf_gamma: f64,
        apf_d_max: i32,
        apf_t_max: usize,
    ) -> &[Action] {
        let n = positions.len();
        let grid_w = grid.width;
        let grid_h = grid.height;
        let cells = (grid_w * grid_h) as usize;

        // Ensure APF field is sized and cleared
        if apf_field.len() != cells {
            *apf_field = vec![0.0; cells];
        } else {
            apf_field.fill(0.0);
        }

        // Initialize priorities (same as one_step_impl)
        if self.priorities.len() != n {
            self.priorities.clear();
            self.priorities.resize(n, 0.0);
            for i in 0..n {
                self.priorities[i] = dist_maps[i].get(positions[i]) as f32;
            }
        }

        self.next_pos_buf.clear();
        self.next_pos_buf.extend_from_slice(positions);
        self.decided_buf.clear();
        self.decided_buf.resize(n, false);

        self.current_occ.reset(grid_w, grid_h);
        for (i, &pos) in positions.iter().enumerate() {
            self.current_occ.set(pos, i);
        }
        self.next_occ.reset(grid_w, grid_h);

        // Pre-decide idle agents and add their APF contribution.
        // Paper assumes all agents contribute to the field, not just tasked ones.
        // Under fault conditions (many immobile agents), this repels tasked agents
        // away from occupied idle positions.
        if !has_task.is_empty() {
            for i in 0..n {
                if !has_task[i] && !self.decided_buf[i] {
                    self.next_pos_buf[i] = positions[i];
                    self.decided_buf[i] = true;
                    self.next_occ.set(positions[i], i);
                    // Idle agent stays in place — add APF at its position
                    add_apf_for_agent(
                        positions[i], goals[i], grid, dist_maps[i],
                        apf_field, grid_w, grid_h, apf_w, apf_gamma, apf_d_max, apf_t_max,
                    );
                }
            }
        }

        // Sort by priority (same as one_step_impl)
        self.order_buf.clear();
        for i in 0..n {
            if !self.decided_buf[i] {
                self.order_buf.push(i);
            }
        }
        let priorities = &self.priorities;
        let task_aware = !has_task.is_empty();
        self.order_buf.sort_unstable_by(|&a, &b| {
            if task_aware {
                let ta = has_task.get(a).copied().unwrap_or(false);
                let tb = has_task.get(b).copied().unwrap_or(false);
                tb.cmp(&ta).then_with(|| {
                    priorities[b]
                        .partial_cmp(&priorities[a])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            } else {
                priorities[b]
                    .partial_cmp(&priorities[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        // Process each agent in priority order.
        // KEY DIFFERENCE from one_step_impl: after each agent commits,
        // project its path and update apf_field so the next agent sees it.
        let order_len = self.order_buf.len();
        for oi in 0..order_len {
            let i = self.order_buf[oi];
            if self.decided_buf[i] {
                continue;
            }

            // Build bias_fn closure that reads from the current apf_field.
            // The field is mutated between iterations, so each agent sees
            // the accumulated APF from all previously-decided agents.
            let apf_ref = &*apf_field;
            let goal_i = goals[i];
            let bias_fn = |pos: IVec2, _agent: usize| -> f64 {
                // Paper: goal cell always has cost 0
                if pos == goal_i {
                    return 0.0;
                }
                let idx = (pos.y * grid_w + pos.x) as usize;
                if idx < apf_ref.len() { apf_ref[idx] } else { 0.0 }
            };

            pibt_assign_grid(
                i,
                &mut self.next_pos_buf,
                &mut self.decided_buf,
                positions,
                goals,
                grid,
                dist_maps,
                &self.priorities,
                0,
                &self.current_occ,
                &mut self.next_occ,
                self.shuffle_seed,
                Some(&bias_fn),
            );

            // After agent i committed: project its future path and add APF.
            // Paper Eq. 11: PIBT_APF_i(v) = sum over t in {0..t_max} of APF_i(v,t)
            let next_pos_i = self.next_pos_buf[i];
            add_apf_for_agent(
                next_pos_i, goals[i], grid, dist_maps[i],
                apf_field, grid_w, grid_h, apf_w, apf_gamma, apf_d_max, apf_t_max,
            );
        }

        // Convert to actions
        self.actions_buf.clear();
        for (pos, next) in positions.iter().zip(self.next_pos_buf.iter()).take(n) {
            self.actions_buf.push(delta_to_action(*pos, *next));
        }

        // Update priorities (match reference: reset to 0 on goal arrival)
        for (i, goal) in goals.iter().enumerate().take(n) {
            if self.next_pos_buf[i] == *goal {
                self.priorities[i] = 0.0;
            } else {
                self.priorities[i] += 1.0;
            }
        }

        self.shuffle_seed = self.shuffle_seed.wrapping_add(1);
        &self.actions_buf
    }

    /// Run one PIBT step with pre-decided constraints.
    ///
    /// `constraints` — list of `(agent_index, target_vertex)` pairs.
    pub fn one_step_constrained(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
        constraints: &[(usize, IVec2)],
    ) -> &[Action] {
        self.one_step_impl(positions, goals, grid, dist_maps, constraints, &[], None)
    }

    /// Internal: full PIBT step with optional constraints, task-awareness, and bias.
    /// Returns a slice borrowing from self.actions_buf (zero allocation after first call).
    fn one_step_impl(
        &mut self,
        positions: &[IVec2],
        goals: &[IVec2],
        grid: &GridMap,
        dist_maps: &[&DistanceMap],
        constraints: &[(usize, IVec2)],
        has_task: &[bool],
        bias_fn: Option<&dyn Fn(IVec2, usize) -> f64>,
    ) -> &[Action] {
        let n = positions.len();

        // Initialize or reinitialize priorities.
        // Must clear before resize: resize(shrink) preserves tail values, so a dead
        // agent's priority would carry over to the agent now occupying that local index.
        if self.priorities.len() != n {
            self.priorities.clear();
            self.priorities.resize(n, 0.0);
            for i in 0..n {
                self.priorities[i] = dist_maps[i].get(positions[i]) as f32;
            }
        }

        // Prepare reusable buffers
        self.next_pos_buf.clear();
        self.next_pos_buf.extend_from_slice(positions);

        self.decided_buf.clear();
        self.decided_buf.resize(n, false);

        self.current_occ.reset(grid.width, grid.height);
        for (i, &pos) in positions.iter().enumerate() {
            self.current_occ.set(pos, i);
        }

        self.next_occ.reset(grid.width, grid.height);

        // Pre-decide constrained agents
        for &(agent, vertex) in constraints {
            if agent < n {
                self.next_pos_buf[agent] = vertex;
                self.decided_buf[agent] = true;
                self.next_occ.set(vertex, agent);
            }
        }

        // Idle agents (has_task == false) are NOT pre-decided: they participate
        // in PIBT normally so higher-priority agents can push them out of the way
        // via priority inheritance. They naturally prefer staying at their goal
        // (BFS distance = 0) but are movable when blocking traffic.

        // Sort UNCONSTRAINED agents by priority (descending).
        // Task-aware: agents with active tasks sort before idle agents
        // (reference PIBT_MAPD behavior — assigned > free).
        self.order_buf.clear();
        for i in 0..n {
            if !self.decided_buf[i] {
                self.order_buf.push(i);
            }
        }
        let priorities = &self.priorities;
        let task_aware = !has_task.is_empty();
        self.order_buf.sort_unstable_by(|&a, &b| {
            if task_aware {
                let ta = has_task.get(a).copied().unwrap_or(false);
                let tb = has_task.get(b).copied().unwrap_or(false);
                // Tasked agents first (true > false when reversed)
                tb.cmp(&ta).then_with(|| {
                    priorities[b]
                        .partial_cmp(&priorities[a])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            } else {
                priorities[b]
                    .partial_cmp(&priorities[a])
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        // Clone order to avoid borrow conflict with &mut self fields
        // (order_buf is read-only during assignment)
        let order_len = self.order_buf.len();
        for oi in 0..order_len {
            let i = self.order_buf[oi];
            if self.decided_buf[i] {
                continue;
            }
            pibt_assign_grid(
                i,
                &mut self.next_pos_buf,
                &mut self.decided_buf,
                positions,
                goals,
                grid,
                dist_maps,
                &self.priorities,
                0,
                &self.current_occ,
                &mut self.next_occ,
                self.shuffle_seed,
                bias_fn,
            );
        }

        // Convert position deltas to actions (reuse buffer)
        self.actions_buf.clear();
        for (pos, next) in positions.iter().zip(self.next_pos_buf.iter()).take(n) {
            self.actions_buf.push(delta_to_action(*pos, *next));
        }

        // Update priorities: match reference PIBT_MAPD behavior.
        // Reference (pibt_mapd.cpp:137):
        //   elapsed = (v_next == g) ? 0 : elapsed + 1
        // Resetting to 0 on goal arrival is essential: it makes the agent
        // low-priority and easily pushable, preventing "goal squatting" where
        // idle agents with accumulated priority block corridors.
        for (i, goal) in goals.iter().enumerate().take(n) {
            if self.next_pos_buf[i] == *goal {
                self.priorities[i] = 0.0;
            } else {
                self.priorities[i] += 1.0;
            }
        }

        // Advance shuffle seed for next step
        self.shuffle_seed = self.shuffle_seed.wrapping_add(1);

        // Return slice borrowing internal buffer — zero allocation
        &self.actions_buf
    }
}

// ---------------------------------------------------------------------------
// Core PIBT one-step function (standalone, allocates fresh — legacy path)
// ---------------------------------------------------------------------------

/// Execute one PIBT timestep without constraints (standalone, allocates fresh).
/// Used by legacy `PibtSolver::solve()` only.
pub fn pibt_one_step(
    positions: &[IVec2],
    goals: &[IVec2],
    grid: &GridMap,
    dist_maps: &[&DistanceMap],
    priorities: &mut [f32],
) -> Vec<Action> {
    pibt_one_step_constrained(positions, goals, grid, dist_maps, priorities, &[])
}

/// Execute one PIBT timestep with constraints (standalone, allocates fresh).
pub fn pibt_one_step_constrained(
    positions: &[IVec2],
    goals: &[IVec2],
    grid: &GridMap,
    dist_maps: &[&DistanceMap],
    priorities: &mut [f32],
    constraints: &[(usize, IVec2)],
) -> Vec<Action> {
    let n = positions.len();
    let mut next_pos = positions.to_vec();
    let mut decided = vec![false; n];

    let mut current_occ = OccGrid::new();
    current_occ.reset(grid.width, grid.height);
    for (i, &pos) in positions.iter().enumerate() {
        current_occ.set(pos, i);
    }

    let mut next_occ = OccGrid::new();
    next_occ.reset(grid.width, grid.height);

    for &(agent, vertex) in constraints {
        if agent < n {
            next_pos[agent] = vertex;
            decided[agent] = true;
            next_occ.set(vertex, agent);
        }
    }

    let mut order: Vec<usize> = (0..n).filter(|i| !decided[*i]).collect();
    order.sort_unstable_by(|&a, &b| {
        priorities[b]
            .partial_cmp(&priorities[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Use a simple step counter for deterministic shuffle in standalone path
    let shuffle_seed = positions.len() as u64;
    for &i in &order {
        if decided[i] {
            continue;
        }
        pibt_assign_grid(
            i,
            &mut next_pos,
            &mut decided,
            positions,
            goals,
            grid,
            dist_maps,
            priorities,
            0,
            &current_occ,
            &mut next_occ,
            shuffle_seed,
            None,
        );
    }

    (0..n)
        .map(|i| delta_to_action(positions[i], next_pos[i]))
        .collect()
}

// ---------------------------------------------------------------------------
// Recursive PIBT assignment (grid-indexed occupation)
// ---------------------------------------------------------------------------

#[allow(clippy::only_used_in_recursion)]
fn pibt_assign_grid(
    agent: usize,
    next_pos: &mut [IVec2],
    decided: &mut [bool],
    current: &[IVec2],
    goals: &[IVec2],
    grid: &GridMap,
    dist_maps: &[&DistanceMap],
    priorities: &[f32],
    depth: usize,
    current_occ: &OccGrid,
    next_occ: &mut OccGrid,
    shuffle_seed: u64,
    bias_fn: Option<&dyn Fn(IVec2, usize) -> f64>,
) -> bool {
    if depth > current.len() {
        next_pos[agent] = current[agent];
        decided[agent] = true;
        next_occ.set(current[agent], agent);
        return false;
    }

    let pos = current[agent];

    let mut candidates = [IVec2::ZERO; 5];
    let mut n_cand = 0usize;
    for dir in Direction::ALL {
        let next = pos + dir.offset();
        if grid.is_walkable(next) {
            candidates[n_cand] = next;
            n_cand += 1;
        }
    }
    candidates[n_cand] = pos;
    n_cand += 1;
    let candidates = &mut candidates[..n_cand];

    // Shuffle before sorting (reference C++ behavior): randomizes among
    // equal-distance candidates, preventing systematic bias in corridors.
    // Use a fast deterministic hash instead of full RNG to avoid allocation.
    let hash_base = shuffle_seed.wrapping_mul(6364136223846793005)
        .wrapping_add(agent as u64)
        .wrapping_add(depth as u64);
    candidates.sort_unstable_by(|&a, &b| {
        let da_raw = dist_maps[agent].get(a);
        let db_raw = dist_maps[agent].get(b);
        // Tie-break helpers (shared by both branches)
        let occ_cmp = || {
            let occ_a = current_occ.get(a).is_some() as u8;
            let occ_b = current_occ.get(b).is_some() as u8;
            occ_a.cmp(&occ_b)
        };
        let hash_cmp = || {
            let ha = hash_base.wrapping_mul(a.x as u64 + 1).wrapping_add(a.y as u64);
            let hb = hash_base.wrapping_mul(b.x as u64 + 1).wrapping_add(b.y as u64);
            ha.cmp(&hb)
        };
        if let Some(bf) = bias_fn {
            // Bias path: sort by (distance + bias) as f64, then original tie-breaks
            let da = da_raw as f64 + bf(a, agent);
            let db = db_raw as f64 + bf(b, agent);
            da.partial_cmp(&db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(occ_cmp)
                .then_with(hash_cmp)
        } else {
            // Original integer path — zero overhead when bias is None
            da_raw.cmp(&db_raw)
                .then_with(occ_cmp)
                .then_with(hash_cmp)
        }
    });

    for &candidate in candidates.iter() {
        if let Some(j) = next_occ.get(candidate)
            && j != agent {
                continue;
            }

        if let Some(j) = next_occ.get(pos)
            && j != agent && current[j] == candidate {
                continue;
            }

        let blocker = current_occ
            .get(candidate)
            .filter(|&j| j != agent && !decided[j]);

        if let Some(blocker_id) = blocker {
            // Reference behavior (pibt_mapd.cpp:230-232): push ANY undecided
            // agent at the target cell. No priority check — the processing
            // order (highest priority first) provides implicit inheritance.
            // The pushed agent cooperatively tries to find another cell.
            next_pos[agent] = candidate;
            decided[agent] = true;
            next_occ.set(candidate, agent);

            if pibt_assign_grid(
                blocker_id, next_pos, decided, current, goals, grid, dist_maps,
                priorities, depth + 1, current_occ, next_occ, shuffle_seed, bias_fn,
            ) {
                return true;
            }

            // Backtrack: blocker couldn't find a valid position
            decided[agent] = false;
            next_occ.remove_if_eq(candidate, agent);
            continue;
        }

        next_pos[agent] = candidate;
        decided[agent] = true;
        next_occ.set(candidate, agent);
        return true;
    }

    next_pos[agent] = pos;
    decided[agent] = true;
    next_occ.set(pos, agent);
    false
}

// ---------------------------------------------------------------------------
// APF field construction (paper-accurate, arXiv:2505.22753v1)
// ---------------------------------------------------------------------------

/// Project agent's path forward from `start` toward `goal` for `t_max` steps,
/// then add exponential APF around each cell on that path.
///
/// Formula (Eq. 4): APF_i(v, t) = w * gamma^(-dist) for dist <= d_max.
/// Summed over all projected timesteps (Eq. 11).
fn add_apf_for_agent(
    start: IVec2,
    goal: IVec2,
    grid: &GridMap,
    dist_map: &DistanceMap,
    apf_field: &mut [f64],
    grid_w: i32,
    grid_h: i32,
    w: f64,
    gamma: f64,
    d_max: i32,
    t_max: usize,
) {
    // 1. Project greedy path forward for t_max steps
    let mut path = Vec::with_capacity(t_max + 1);
    path.push(start);
    let mut pos = start;
    for _ in 0..t_max {
        if pos == goal {
            break;
        }
        let mut best = pos;
        let mut best_d = dist_map.get(pos);
        for dir in Direction::ALL {
            let next = pos + dir.offset();
            if grid.is_walkable(next) {
                let d = dist_map.get(next);
                if d < best_d {
                    best_d = d;
                    best = next;
                }
            }
        }
        if best == pos {
            break;
        }
        pos = best;
        path.push(pos);
    }

    // 2. For each cell on the projected path, add APF within d_max radius.
    //    Formula: w * gamma^(-dist) where dist = Manhattan distance.
    for &path_pos in &path {
        for dy in -d_max..=d_max {
            for dx in -d_max..=d_max {
                let p = path_pos + IVec2::new(dx, dy);
                if p.x < 0 || p.x >= grid_w || p.y < 0 || p.y >= grid_h {
                    continue;
                }
                let dist = (dx.abs() + dy.abs()) as i32;
                if dist > d_max {
                    continue;
                }
                let idx = (p.y * grid_w + p.x) as usize;
                if idx < apf_field.len() {
                    // w * gamma^(-dist) = w / gamma^dist
                    apf_field[idx] += w * gamma.powi(-dist);
                }
            }
        }
    }
}
