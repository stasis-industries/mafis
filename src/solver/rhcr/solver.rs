//! RHCR — Rolling-Horizon Collision Resolution (PBS variant)
//!
//! Windowed lifelong solver that replans every W ticks for a horizon of H steps.
//! Uses Priority-Based Search as the inner planner. Falls back to PIBT for
//! agents the inner planner fails to handle.
//!
//! REFERENCE: docs/papers_codes/rhcr/ (Jiaoyang-Li/RHCR)
//! Paper: Li, Tinka, Kiesel, Durham, Kumar, Koenig — "Lifelong Multi-Agent
//! Path Finding in Large-Scale Warehouses", AAAI 2021.
//!
//! Audited 2026-04-06. Specific reference mappings:
//! - `update_congestion()` → `BasicSystem::congested()` at BasicSystem.cpp:310.
//!   Both use the >50% wait threshold over a simulation window. MAFIS extends
//!   this with per-cell exponentially-decayed wait counts for congestion-aware
//!   routing — a small additive feature, not a deviation from the canonical
//!   detection rule.
//! - `fill_goal_sequences()` → `KivaSystem::update_goal_locations()` at
//!   KivaSystem.cpp:70. MAFIS uses a simplified version: append random endpoints
//!   until cumulative distance reaches the horizon. The reference's
//!   `hold_endpoints` semantics are not implemented because MAFIS's task
//!   lifecycle (TaskLeg state machine in src/core/task/) handles endpoint
//!   reservations differently.
//!
//! See `pbs_planner.rs` for inner-planner deviations from `PBS.cpp`.
//!
//! The MAFIS RHCR-PBS, PIBT-Window, and Priority-A* "modes" that previously
//! coexisted under a strategy enum were MAFIS-internal extrapolations not
//! present in the canonical paper, and have been archived on
//! `archive/cut-solvers`. Only PBS remains, matching Li et al.'s default
//! configuration.

use bevy::prelude::*;
use smallvec::smallvec;

use crate::constants;
use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::{RandomScheduler, TaskScheduler};
use crate::core::topology::ZoneMap;

use super::windowed::{WindowAgent, WindowContext, WindowResult, WindowedPlanner};
use crate::solver::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use crate::solver::shared::heuristics::DistanceMapCache;
use crate::solver::shared::pibt_core::PibtCore;
use crate::solver::shared::traits::{Optimality, Scalability, SolverInfo};

use super::pbs_planner::PbsPlanner;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RhcrConfig {
    pub horizon: usize,
    pub replan_interval: usize,
    pub pbs_node_limit: usize,
}

impl RhcrConfig {
    /// Compute smart defaults from grid dimensions and agent count.
    ///
    /// Key trade-off: longer horizons = better plans, but larger frame stalls.
    /// We cap H to keep worst-case PBS replan under ~50ms in WASM.
    pub fn auto(grid_area: usize, num_agents: usize) -> Self {
        let density = if grid_area > 0 { num_agents as f32 / grid_area as f32 } else { 0.0 };

        // PBS tree search is exponential — keep horizon small.
        let mode_max_h = 15;

        // H: scale with grid size, but cap by density and mode
        let base_h = (grid_area as f32).sqrt().ceil() as usize;
        let h = if density > 0.15 {
            base_h.min(10)
        } else if density > 0.05 {
            base_h.min(15)
        } else {
            base_h.min(mode_max_h)
        };
        let h = h.clamp(constants::RHCR_MIN_HORIZON, constants::RHCR_MAX_HORIZON);

        // W: replan more often to spread cost across ticks.
        let w = if density > 0.15 {
            (h / 4).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else if density > 0.05 {
            (h / 3).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else {
            (h / 2).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        };

        // Tighter node limit for PBS — reduces worst-case tree explosion
        let pbs_node_limit = (num_agents * 3).clamp(50, constants::PBS_MAX_NODE_LIMIT);

        Self { horizon: h, replan_interval: w, pbs_node_limit }
    }
}

// ---------------------------------------------------------------------------
// RHCR Solver
// ---------------------------------------------------------------------------

pub struct RhcrSolver {
    config: RhcrConfig,
    planner: Box<dyn WindowedPlanner>,
    pibt_fallback: PibtCore,
    ticks_since_replan: usize,
    plan_buffer: Vec<AgentPlan>,
    /// Previous plans from last successful replan, for warm-starting the next window.
    /// Stored as `SmallVec` matching `PlanFragment.actions` storage so the
    /// per-replan write at the bottom of `step()` is a cheap inline clone
    /// (zero heap allocations when plans fit in the 20-action inline buffer,
    /// which is the common case for the default horizon).
    previous_plans: Vec<(usize, smallvec::SmallVec<[Action; 20]>)>,
    /// Previous positions for congestion detection (reference C++ BasicSystem::congested).
    prev_positions: Vec<IVec2>,
    /// Consecutive ticks where >50% of agents are stuck.
    congestion_streak: usize,
    /// Per-cell exponentially-decayed wait counts for congestion-aware routing.
    /// Indexed by `y * grid_width + x`. Tracks how often agents wait at each cell.
    wait_counts: Vec<f32>,
    /// Grid width for indexing into `wait_counts`.
    wait_counts_width: i32,
    // ─── Per-replan scratch buffers (Phase 2 hoisting) ────────────────
    //
    // Cleared and re-extended on every replan instead of allocated fresh.
    // Bounded by `num_agents`. The reason these can live on the solver
    // (rather than being function-local) is split-borrow: `WindowContext`
    // borrows these scratch fields immutably while `self.planner` is
    // borrowed mutably — disjoint fields, so the borrow checker accepts.
    scratch_window_agents: Vec<WindowAgent>,
    scratch_initial_plans: Vec<Option<Vec<Action>>>,
    scratch_start_constraints: Vec<(IVec2, u64)>,
}

impl RhcrSolver {
    pub fn new(config: RhcrConfig) -> Self {
        let planner: Box<dyn WindowedPlanner> = Box::new(PbsPlanner::new());

        // Start at replan_interval - 1 so the very first step() triggers a replan.
        // Without this, agents would sit idle for W ticks before getting any plans.
        let initial_counter = config.replan_interval.saturating_sub(1);

        Self {
            config,
            planner,
            pibt_fallback: PibtCore::new(),
            ticks_since_replan: initial_counter,
            plan_buffer: Vec::new(),
            previous_plans: Vec::new(),
            prev_positions: Vec::new(),
            wait_counts: Vec::new(),
            wait_counts_width: 0,
            congestion_streak: 0,
            scratch_window_agents: Vec::new(),
            scratch_initial_plans: Vec::new(),
            scratch_start_constraints: Vec::new(),
        }
    }

    /// Construct an RHCR solver with **pre-sized PBS scratch buffers**. The
    /// underlying [`PbsPlanner::with_capacity`] reserves the full
    /// `FlatConstraintIndex` / `SeqGoalGrid` / `FlatCAT` slabs at construction
    /// (~3 MB total for a 1000-cell × 20-horizon grid) so the first
    /// `plan_window` call doesn't stall the WASM main thread.
    ///
    /// Use this from production paths that know the grid dimensions ahead of
    /// time (the `lifelong_solver_from_name_sized` factory). Tests and the
    /// experiment runner can keep using [`RhcrSolver::new`].
    pub fn with_grid(config: RhcrConfig, grid_w: usize, grid_h: usize) -> Self {
        let max_goals = constants::PBS_GOAL_SEQUENCE_MAX_LEN + 1;
        let planner: Box<dyn WindowedPlanner> =
            Box::new(PbsPlanner::with_capacity(grid_w, grid_h, config.horizon, max_goals));

        let initial_counter = config.replan_interval.saturating_sub(1);

        Self {
            config,
            planner,
            pibt_fallback: PibtCore::new(),
            ticks_since_replan: initial_counter,
            plan_buffer: Vec::new(),
            previous_plans: Vec::new(),
            prev_positions: Vec::new(),
            wait_counts: Vec::new(),
            wait_counts_width: 0,
            congestion_streak: 0,
            scratch_window_agents: Vec::new(),
            scratch_initial_plans: Vec::new(),
            scratch_start_constraints: Vec::new(),
        }
    }

    pub fn config(&self) -> &RhcrConfig {
        &self.config
    }

    /// Populate `WindowAgent.goal_sequence` for every agent by querying the
    /// scheduler's `peek_task_chain`. The chain is the agent's *hypothetical*
    /// future-task projection — alternating pickup/delivery endpoints starting
    /// after `agent.goal`, sized so cumulative Manhattan distance is bounded by
    /// `horizon` and length is bounded by `PBS_GOAL_SEQUENCE_MAX_LEN`.
    ///
    /// Reference: `KivaSystem::update_goal_locations()` (KivaSystem.cpp:70).
    ///
    /// **No `dist_to_goal >= horizon` skip**. Even agents whose primary goal
    /// is far from `agent.pos` get a chain — sequential A* in `plan_agent`
    /// uses the chain to make horizon-bounded best-effort progress. The
    /// previous early-skip was the root cause of PBS's chronic NoSolution
    /// failures on warehouse_large (PAAMS 2026 fix).
    ///
    /// **Scheduler instance**: this routine constructs a local
    /// `RandomScheduler` instead of accepting a `&dyn TaskScheduler` from the
    /// runner. The constraint is that `RhcrSolver::step()` is reached via the
    /// `LifelongSolver` trait, whose signature is locked outside this stream's
    /// editable scope. Using a unit-struct `RandomScheduler` is zero-cost
    /// (the trait method dispatches statically) and gives the same alternation
    /// semantics as the canonical reference. A future stream may plumb the
    /// active scheduler reference through `SolverContext` so the locality-
    /// aware `ClosestFirstScheduler::peek_task_chain` can be used instead.
    fn fill_goal_sequences(
        window_agents: &mut [WindowAgent],
        agent_states: &[AgentState],
        zones: &ZoneMap,
        horizon: usize,
        rng: &mut SeededRng,
    ) {
        // Local zero-sized scheduler instance — no allocation, no plumbing.
        let scheduler = RandomScheduler;

        for (i, wa) in window_agents.iter_mut().enumerate() {
            wa.goal_sequence.clear();
            let task_leg = agent_states.get(i).map(|s| s.task_leg.clone()).unwrap_or_default();
            let chain = scheduler.peek_task_chain(
                zones,
                wa.pos,
                wa.goal,
                task_leg,
                horizon as u64,
                &mut rng.rng,
            );
            wa.goal_sequence.extend(chain.into_iter());
        }
    }

    /// Check congestion: returns true if >50% of agents didn't move since last tick.
    /// Reference: BasicSystem::congested() in the RHCR C++ codebase.
    /// Also updates per-cell `wait_counts` for congestion-aware routing.
    fn update_congestion(&mut self, agents: &[AgentState], grid: &GridMap) -> bool {
        if agents.is_empty() {
            self.congestion_streak = 0;
            return false;
        }

        let n = agents.len();
        let grid_width = grid.width;

        // Initialize or resize wait_counts if grid changed
        let grid_size = (grid.width * grid.height) as usize;
        if self.wait_counts.len() != grid_size || self.wait_counts_width != grid_width {
            self.wait_counts = vec![0.0; grid_size];
            self.wait_counts_width = grid_width;
        }

        if self.prev_positions.len() != n {
            // First call or agent count changed — initialize
            self.prev_positions = agents.iter().map(|a| a.pos).collect();
            self.congestion_streak = 0;
            return false;
        }

        let stuck_count = agents
            .iter()
            .enumerate()
            .filter(|(i, a)| {
                a.pos == self.prev_positions[*i] && a.goal.is_some() && a.pos != a.goal.unwrap()
            })
            .count();

        // Decay existing wait counts
        for w in &mut self.wait_counts {
            *w *= 0.99;
        }

        // Increment cells where agents waited (had a goal but didn't move)
        for (i, a) in agents.iter().enumerate() {
            if a.goal.is_some() && a.pos == self.prev_positions[i] && a.pos != a.goal.unwrap() {
                let idx = (a.pos.y * grid_width + a.pos.x) as usize;
                if idx < self.wait_counts.len() {
                    self.wait_counts[idx] += 1.0;
                }
            }
        }

        // Update positions for next tick
        for (i, a) in agents.iter().enumerate() {
            self.prev_positions[i] = a.pos;
        }

        let congested = stuck_count * 2 > n; // >50% stuck
        if congested {
            self.congestion_streak += 1;
        } else {
            self.congestion_streak = 0;
        }
        congested
    }

    /// Effective replan interval, shortened during congestion.
    /// When congested, replan every 2 ticks (minimum) to break deadlocks faster.
    fn effective_replan_interval(&self) -> usize {
        if self.congestion_streak >= 2 {
            // Congested for 2+ ticks: replan aggressively
            constants::RHCR_MIN_REPLAN_INTERVAL
        } else {
            self.config.replan_interval
        }
    }

    /// LRA-style conflict resolution: takes a set of multi-step plans (possibly
    /// with conflicts) and resolves them by inserting Wait actions for
    /// lower-priority agents. Processes timestep by timestep. When two agents
    /// would collide, the lower-indexed agent waits. This preserves overall plan
    /// structure while eliminating collisions.
    ///
    /// Returns resolved plans as `Vec<AgentPlan>`.
    /// Priority rule: agents earlier in `plans` = higher priority (keep their
    /// planned action). This matches MAFIS convention where lower agent index
    /// means higher priority.
    fn lra_resolve_conflicts(
        plans: &[(usize, Vec<Action>)],
        agents: &[AgentState],
        _grid: &GridMap,
        max_steps: usize,
    ) -> Vec<AgentPlan> {
        use smallvec::SmallVec;

        let n = plans.len();
        if n == 0 {
            return Vec::new();
        }

        // Current position per slot (initialized from AgentState)
        let mut cur_pos: Vec<IVec2> = plans
            .iter()
            .map(|(idx, _)| {
                agents.iter().find(|a| a.index == *idx).map(|a| a.pos).unwrap_or(IVec2::ZERO)
            })
            .collect();

        // Cursor into each agent's original plan (how far we've consumed)
        let mut cursors: Vec<usize> = vec![0; n];

        // Resolved actions per slot
        let mut resolved: Vec<SmallVec<[Action; 20]>> = vec![SmallVec::new(); n];

        // Per-iteration scratch buffers — hoisted out of the timestep loop so
        // they allocate once per LRA call instead of `max_steps` times. Each
        // iteration `clear()`s and re-pushes; the underlying capacity is
        // reused across iterations, so the inner loop becomes allocation-free
        // after the first iteration.
        let mut intended_action: Vec<Action> = Vec::with_capacity(n);
        let mut intended_pos: Vec<IVec2> = Vec::with_capacity(n);
        let mut forced_wait: Vec<bool> = Vec::with_capacity(n);

        for _t in 0..max_steps {
            // Phase 1: compute intended next position for each slot
            intended_action.clear();
            intended_pos.clear();

            for slot in 0..n {
                let (_, ref plan) = plans[slot];
                let action =
                    if cursors[slot] < plan.len() { plan[cursors[slot]] } else { Action::Wait };
                intended_action.push(action);
                intended_pos.push(action.apply(cur_pos[slot]));
            }

            // Phase 2: detect conflicts and force lower-priority agents to Wait.
            // We iterate in priority order (slot 0 = highest priority). An agent
            // forced to Wait does NOT advance its cursor.
            forced_wait.clear();
            forced_wait.resize(n, false);

            // Vertex conflicts: two agents want to occupy the same cell at t+1.
            // The lower-priority one (higher slot index) waits.
            for hi in 0..n {
                if forced_wait[hi] {
                    continue;
                }
                for lo in (hi + 1)..n {
                    if forced_wait[lo] {
                        continue;
                    }
                    if intended_pos[hi] == intended_pos[lo] {
                        // lo has lower priority -> force wait
                        forced_wait[lo] = true;
                        intended_action[lo] = Action::Wait;
                        intended_pos[lo] = cur_pos[lo]; // stay put
                    }
                }
            }

            // Edge conflicts (swaps): agent A moves to B's old cell while B moves
            // to A's old cell. Force the lower-priority one to Wait.
            for hi in 0..n {
                if forced_wait[hi] {
                    continue;
                }
                for lo in (hi + 1)..n {
                    if forced_wait[lo] {
                        continue;
                    }
                    let swapped = intended_pos[hi] == cur_pos[lo]
                        && intended_pos[lo] == cur_pos[hi]
                        && intended_pos[hi] != cur_pos[hi]; // actual movement
                    if swapped {
                        forced_wait[lo] = true;
                        intended_action[lo] = Action::Wait;
                        intended_pos[lo] = cur_pos[lo];
                    }
                }
            }

            // After forcing waits, re-check for NEW vertex conflicts caused by
            // a waited agent now staying at a cell that another agent moves into.
            // We do a second pass in priority order.
            for hi in 0..n {
                for lo in (hi + 1)..n {
                    if forced_wait[lo] {
                        // lo is already waiting — check if hi is moving INTO lo's
                        // current position (which lo is now staying at)
                        if !forced_wait[hi] && intended_pos[hi] == intended_pos[lo] {
                            forced_wait[hi] = true;
                            intended_action[hi] = Action::Wait;
                            intended_pos[hi] = cur_pos[hi];
                        }
                    }
                }
            }

            // Phase 3: commit actions, advance cursors for non-waited agents
            for slot in 0..n {
                resolved[slot].push(intended_action[slot]);
                cur_pos[slot] = intended_pos[slot];
                if !forced_wait[slot] && cursors[slot] < plans[slot].1.len() {
                    cursors[slot] += 1;
                }
            }

            // Early exit: all cursors exhausted
            if cursors.iter().enumerate().all(|(s, &c)| c >= plans[s].1.len()) {
                break;
            }
        }

        // Build output: trim trailing Waits for compactness
        plans
            .iter()
            .enumerate()
            .map(|(slot, (idx, _))| {
                let mut actions = std::mem::take(&mut resolved[slot]);
                // Trim trailing Waits (but keep at least one action)
                while actions.len() > 1 && actions.last() == Some(&Action::Wait) {
                    actions.pop();
                }
                (*idx, actions)
            })
            .collect()
    }

    /// Apply PIBT fallback to a set of agent indices.
    /// `solved_first_steps` contains (agent_index, next_step_target) for
    /// agents whose plans were kept — PIBT must not collide with them.
    /// `solved_current_positions` contains the current cell of each solved agent —
    /// included so PIBT's occupation grid doesn't ignore them as "blockers".
    fn pibt_fallback_for(
        &mut self,
        failed_indices: &[usize],
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        grid: &GridMap,
        solved_first_steps: &[(usize, IVec2)],
    ) {
        if failed_indices.is_empty() {
            return;
        }

        // Build sub-problem for failed agents only
        let sub_agents: Vec<&AgentState> = failed_indices
            .iter()
            .filter_map(|&idx| agents.iter().find(|a| a.index == idx))
            .collect();

        if sub_agents.is_empty() {
            return;
        }

        // Build combined positions for PIBT: failed agents first, then solved agents as "dummy"
        // agents that stay in place. This makes PibtCore's occupation grid aware of solved
        // agents' current cells, preventing failed agents from moving into them.
        let solved_current: Vec<IVec2> = solved_first_steps
            .iter()
            .filter_map(|&(idx, _)| agents.iter().find(|a| a.index == idx).map(|a| a.pos))
            .collect();

        let mut all_positions: Vec<IVec2> = sub_agents.iter().map(|a| a.pos).collect();
        let mut all_goals: Vec<IVec2> =
            sub_agents.iter().map(|a| a.goal.unwrap_or(a.pos)).collect();

        // Dummy solved agents: goal == current pos so they always plan Wait
        all_positions.extend_from_slice(&solved_current);
        all_goals.extend_from_slice(&solved_current);

        // Single cache call for all agents (failed + dummy solved)
        let all_pairs: Vec<(IVec2, IVec2)> =
            all_positions.iter().zip(all_goals.iter()).map(|(&p, &g)| (p, g)).collect();
        let all_dist_maps = distance_cache.get_or_compute(grid, &all_pairs);

        let n_failed = sub_agents.len();
        let actions = self.pibt_fallback.one_step(&all_positions, &all_goals, grid, &all_dist_maps);

        // Collect next-step targets of solved agents (first step or current pos if empty plan)
        let solved_targets: std::collections::HashSet<IVec2> =
            solved_first_steps.iter().map(|&(_, t)| t).collect();

        // Only apply the first n_failed actions (ignore dummy solved agents)
        for (i, &action) in actions.iter().take(n_failed).enumerate() {
            let target = action.apply(sub_agents[i].pos);
            if solved_targets.contains(&target) {
                // Collision with solved agent's next position — force wait
                self.plan_buffer.push((sub_agents[i].index, smallvec![Action::Wait]));
            } else {
                self.plan_buffer.push((sub_agents[i].index, smallvec![action]));
            }
        }
    }
}

impl LifelongSolver for RhcrSolver {
    fn name(&self) -> &'static str {
        "rhcr_pbs"
    }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(W⁻¹ × PBS_node_limit) amortized per tick",
            scalability: Scalability::Medium,
            description: "RHCR (PBS) — windowed planning with Priority-Based Search. High quality, bounded by node limit.",
            source: "Li et al., AAAI 2021",
            recommended_max_agents: Some(200),
        }
    }

    fn reset(&mut self) {
        self.ticks_since_replan = self.config.replan_interval.saturating_sub(1);
        self.pibt_fallback.reset();
        self.planner.reset();
        self.plan_buffer.clear();
        self.previous_plans.clear();
        self.prev_positions.clear();
        self.congestion_streak = 0;
        self.wait_counts.clear();
        self.wait_counts_width = 0;
        self.scratch_window_agents.clear();
        self.scratch_initial_plans.clear();
        self.scratch_start_constraints.clear();
    }

    fn save_priorities(&self) -> Vec<f32> {
        // Save both fallback PIBT and windowed planner priorities.
        // Format: [fallback_len, fallback_priorities..., planner_priorities...]
        let fb = self.pibt_fallback.priorities();
        let wp = self.planner.save_priorities();
        let mut out = Vec::with_capacity(1 + fb.len() + wp.len());
        out.push(fb.len() as f32);
        out.extend_from_slice(fb);
        out.extend_from_slice(&wp);
        out
    }

    fn restore_priorities(&mut self, priorities: &[f32]) {
        if priorities.is_empty() {
            return;
        }
        let fb_len = priorities[0] as usize;
        let rest = &priorities[1..];
        if rest.len() >= fb_len {
            self.pibt_fallback.set_priorities(&rest[..fb_len]);
            self.planner.restore_priorities(&rest[fb_len..]);
        }
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        self.ticks_since_replan += 1;

        // Sync PIBT fallback shuffle seed with tick for deterministic tie-breaking after rewind.
        self.pibt_fallback.set_shuffle_seed(ctx.tick);

        // Update congestion detection (reference C++ BasicSystem::congested)
        self.update_congestion(agents, ctx.grid);

        // Force replan when any agent has no plan (goal changed mid-window).
        // Require a minimum cooldown of 2 ticks to avoid replanning every tick
        // when goals change rapidly (e.g., high-throughput warehouses).
        let force_replan = self.ticks_since_replan >= 2
            && agents.iter().any(|a| !a.has_plan && a.goal.is_some() && a.pos != a.goal.unwrap());

        // Dynamic replan interval: shorten during congestion to break deadlocks
        let effective_interval = self.effective_replan_interval();

        // Not time to replan yet — zero-cost path
        if !force_replan && self.ticks_since_replan < effective_interval {
            return StepResult::Continue;
        }

        // Time to replan
        self.ticks_since_replan = 0;
        self.plan_buffer.clear();

        if agents.is_empty() {
            return StepResult::Replan(&self.plan_buffer);
        }

        // Build WindowAgent list into the scratch buffer (reused across replans).
        self.scratch_window_agents.clear();
        self.scratch_window_agents.extend(agents.iter().map(|a| WindowAgent {
            index: a.index,
            pos: a.pos,
            goal: a.goal.unwrap_or(a.pos),
            goal_sequence: smallvec::SmallVec::new(),
        }));

        // Note: distance maps are no longer pre-computed here. PbsPlanner
        // populates the persistent `distance_cache` itself with the
        // augmented goal set (primary + peek-chain) at the top of
        // `plan_window`, then queries it via `get_cached(goal)` inside the
        // PBS loop. The previous per-tick `get_or_compute(...)` here held an
        // immutable borrow on `distance_cache` that would conflict with the
        // `&mut distance_cache` argument to `plan_window`.

        // Fill goal sequences (reference: KivaSystem::update_goal_locations).
        // No `dist_to_goal >= horizon` skip — see `fill_goal_sequences` doc
        // comment for the rationale (PAAMS 2026 PBS throughput fix).
        Self::fill_goal_sequences(
            &mut self.scratch_window_agents,
            agents,
            ctx.zones,
            self.config.horizon,
            rng,
        );

        // Build initial plans for warm-starting: reuse previous plans for agents
        // whose goal hasn't changed and whose plan is still valid (all cells walkable).
        // Reuse the scratch buffer instead of allocating a fresh Vec per replan.
        self.scratch_initial_plans.clear();
        self.scratch_initial_plans.resize(self.scratch_window_agents.len(), None);
        for (prev_idx, prev_actions) in &self.previous_plans {
            // Find the agent in the current window
            if let Some(local_i) =
                self.scratch_window_agents.iter().position(|a| a.index == *prev_idx)
            {
                let agent = &self.scratch_window_agents[local_i];
                // Only reuse if plan is non-empty and still valid from current position:
                // every intermediate cell must be walkable and the plan must reach the goal.
                if !prev_actions.is_empty() {
                    let mut pos = agent.pos;
                    let mut valid = true;
                    let mut reaches_goal = false;
                    for action in prev_actions {
                        pos = action.apply(pos);
                        if !ctx.grid.is_walkable(pos) {
                            valid = false;
                            break;
                        }
                        if pos == agent.goal {
                            reaches_goal = true;
                        }
                    }
                    if valid && reaches_goal {
                        // Convert SmallVec → Vec for WindowContext.initial_plans
                        // type compatibility. This is the same allocation cost
                        // as the previous Vec→Vec clone — the win from the
                        // SmallVec storage change is on the *write* side at
                        // the bottom of this function.
                        self.scratch_initial_plans[local_i] = Some(prev_actions.to_vec());
                    }
                }
            }
        }

        // Cross-window start constraints: vertex constraints at t=0 for all
        // agents' start positions, preventing agent A from planning to be at
        // agent B's position at t=0 when B hasn't moved yet. Reused scratch
        // buffer (cleared+extended per replan).
        self.scratch_start_constraints.clear();
        self.scratch_start_constraints
            .extend(self.scratch_window_agents.iter().map(|a| (a.pos, 0u64)));

        let window_ctx = WindowContext {
            grid: ctx.grid,
            horizon: self.config.horizon,
            node_limit: self.config.pbs_node_limit,
            agents: &self.scratch_window_agents,
            distance_maps: &[],
            initial_plans: &self.scratch_initial_plans,
            start_constraints: &self.scratch_start_constraints,
            travel_penalties: &self.wait_counts,
        };

        // Run windowed planner. The persistent simulation cache is passed
        // through so PBS can populate it with the augmented goal set
        // (primary + peek-chain) once and reuse those distance maps across
        // every branch of the PBS DFS — no per-window allocation churn.
        let result = self.planner.plan_window(&window_ctx, distance_cache, rng);

        match result {
            WindowResult::Solved(fragments) => {
                for frag in fragments {
                    self.plan_buffer.push((frag.agent_index, frag.actions));
                }
            }
            WindowResult::Partial { solved, failed } => {
                // PBS uses per-agent LRA fallback: combine solved multi-step plans
                // with single-Wait stubs for failed agents, then resolve
                // conflicts timestep-by-timestep. This preserves plan structure
                // (unlike a full-PIBT fallback which would discard everything).
                let mut combined: Vec<(usize, Vec<Action>)> =
                    Vec::with_capacity(solved.len() + failed.len());
                let solved_first_steps: Vec<(usize, IVec2)> = solved
                    .iter()
                    .filter_map(|frag| {
                        let agent = agents.iter().find(|a| a.index == frag.agent_index)?;
                        let target = frag
                            .actions
                            .first()
                            .map(|action| action.apply(agent.pos))
                            .unwrap_or(agent.pos);
                        Some((frag.agent_index, target))
                    })
                    .collect();
                for frag in &solved {
                    combined.push((frag.agent_index, frag.actions.to_vec()));
                }
                for &idx in &failed {
                    combined.push((idx, vec![Action::Wait]));
                }

                let resolved =
                    Self::lra_resolve_conflicts(&combined, agents, ctx.grid, self.config.horizon);

                // LRA may produce all-Wait plans for stuck agents on tight maps.
                // Fall back to PIBT for those agents.
                let mut pibt_needed: Vec<usize> = Vec::new();
                for (idx, actions) in &resolved {
                    if actions.iter().all(|a| *a == Action::Wait) && failed.contains(idx) {
                        pibt_needed.push(*idx);
                    } else {
                        self.plan_buffer.push((*idx, actions.iter().copied().collect()));
                    }
                }
                if !pibt_needed.is_empty() {
                    self.pibt_fallback_for(
                        &pibt_needed,
                        agents,
                        distance_cache,
                        ctx.grid,
                        &solved_first_steps,
                    );
                }
            }
        }

        // Store plans for warm-starting next replan. The clone is a cheap
        // inline copy when the SmallVec is in inline mode (≤20 actions, the
        // common case for the default horizon) — zero heap allocations.
        self.previous_plans.clear();
        for (idx, actions) in &self.plan_buffer {
            self.previous_plans.push((*idx, actions.clone()));
        }

        StepResult::Replan(&self.plan_buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::task::TaskLeg;
    use crate::core::topology::ZoneMap;
    use crate::solver::shared::heuristics::DistanceMapCache;
    use std::collections::HashMap;

    fn test_zones() -> ZoneMap {
        ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(4, 4)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        }
    }

    #[test]
    fn rhcr_first_call_replans_then_continues() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig { horizon: 10, replan_interval: 5, pbs_node_limit: 100 };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };

        // First call should replan immediately (counter starts at W-1)
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));

        // After replan, agent has a plan — simulate ECS applying it.
        for tick in 1..=4 {
            let agents_moving = vec![AgentState {
                index: 0,
                pos: IVec2::new(tick, 0),
                goal: Some(IVec2::new(4, 4)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            }];
            let result = solver.step(&ctx, &agents_moving, &mut cache, &mut rng);
            assert!(matches!(result, StepResult::Continue), "tick {} should be Continue", tick);
        }

        // 5th call after replan should replan again
        let agents_at_4 = vec![AgentState {
            index: 0,
            pos: IVec2::new(4, 1),
            goal: Some(IVec2::new(4, 4)),
            has_plan: true,
            task_leg: TaskLeg::Free,
        }];
        let result = solver.step(&ctx, &agents_at_4, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));
    }

    #[test]
    fn rhcr_auto_config_scales() {
        // Small grid, few agents
        let cfg = RhcrConfig::auto(100, 5);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
        assert!(cfg.replan_interval >= constants::RHCR_MIN_REPLAN_INTERVAL);
        assert!(cfg.replan_interval <= cfg.horizon);

        // Large grid, many agents
        let cfg = RhcrConfig::auto(16384, 400);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
    }

    #[test]
    fn rhcr_pbs_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig { horizon: 15, replan_interval: 1, pbs_node_limit: 500 };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(4, 0)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
            AgentState {
                index: 1,
                pos: IVec2::new(0, 4),
                goal: Some(IVec2::new(4, 4)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
        ];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        match result {
            StepResult::Replan(plans) => {
                assert_eq!(plans.len(), 2);
                assert!(!plans[0].1.is_empty());
                assert!(!plans[1].1.is_empty());
            }
            _ => panic!("expected Replan"),
        }
    }

    #[test]
    fn rhcr_force_replans_when_agent_has_no_plan() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig { horizon: 10, replan_interval: 5, pbs_node_limit: 100 };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        // Initial replan
        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));

        // Normal continue (agent has plan, not at goal)
        let agents_with_plan = vec![AgentState {
            index: 0,
            pos: IVec2::new(1, 0),
            goal: Some(IVec2::new(4, 4)),
            has_plan: true,
            task_leg: TaskLeg::TravelEmpty(IVec2::new(4, 4)),
        }];
        let result = solver.step(&ctx, &agents_with_plan, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Continue));

        // Simulate goal change: agent reached old goal, got new goal, plan cleared
        let agents_new_goal = vec![AgentState {
            index: 0,
            pos: IVec2::new(4, 4),
            goal: Some(IVec2::new(0, 0)),
            has_plan: false,
            task_leg: TaskLeg::TravelLoaded { from: IVec2::new(4, 4), to: IVec2::ZERO },
        }];
        // Should force-replan even though W ticks haven't elapsed
        let result = solver.step(&ctx, &agents_new_goal, &mut cache, &mut rng);
        assert!(
            matches!(result, StepResult::Replan(_)),
            "RHCR should force-replan when agent has no plan and hasn't reached goal"
        );
    }

    /// Smoke test: RHCR-PBS on a real warehouse with goal recycling — no obstacle
    /// violations, no panics. Smaller agent count to keep test fast.
    #[test]
    fn rhcr_warehouse_lifelong_smoke() {
        use crate::core::topology::TopologyRegistry;
        use rand::Rng;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid, zones) = TopologyRegistry::parse_entry(entry).unwrap();

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 8;
        let ticks = 100u64;

        let mut rng_seed = ChaCha8Rng::seed_from_u64(99);
        let mut positions: Vec<IVec2> = walkable[..n].to_vec();
        let mut goals: Vec<IVec2> =
            (0..n).map(|_| walkable[rng_seed.random_range(0..walkable.len())]).collect();
        let mut plans: Vec<std::collections::VecDeque<Action>> =
            vec![std::collections::VecDeque::new(); n];

        let config = RhcrConfig::auto((grid.width * grid.height) as usize, n);
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        for tick in 0..ticks {
            for i in 0..n {
                let action = plans[i].pop_front().unwrap_or(Action::Wait);
                let new_pos = action.apply(positions[i]);
                assert!(
                    grid.is_walkable(new_pos),
                    "RHCR-PBS: agent {} tick {} moved to obstacle {:?}",
                    i,
                    tick,
                    new_pos,
                );
                positions[i] = new_pos;
            }

            for i in 0..n {
                if positions[i] == goals[i] {
                    goals[i] = walkable[rng_seed.random_range(0..walkable.len())];
                    plans[i].clear();
                }
            }

            let agent_states: Vec<AgentState> = (0..n)
                .map(|i| AgentState {
                    index: i,
                    pos: positions[i],
                    goal: Some(goals[i]),
                    has_plan: !plans[i].is_empty(),
                    task_leg: TaskLeg::Free,
                })
                .collect();

            let solver_zones = ZoneMap {
                pickup_cells: zones.pickup_cells.clone(),
                delivery_cells: zones.delivery_cells.clone(),
                corridor_cells: zones.corridor_cells.clone(),
                recharging_cells: Vec::new(),
                zone_type: zones.zone_type.clone(),
                queue_lines: Vec::new(),
            };
            let ctx = SolverContext { grid: &grid, zones: &solver_zones, tick, num_agents: n };

            match solver.step(&ctx, &agent_states, &mut cache, &mut rng) {
                StepResult::Continue => {}
                StepResult::Replan(new_plans) => {
                    for (idx, actions) in new_plans {
                        if *idx < n {
                            plans[*idx] = actions.iter().copied().collect();
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rhcr_handles_empty_agent_list() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig::auto(25, 0);
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 0 };
        let result = solver.step(&ctx, &[], &mut cache, &mut rng);
        match result {
            StepResult::Replan(plans) => assert_eq!(plans.len(), 0),
            _ => panic!("expected Replan with empty plans"),
        }
    }

    #[test]
    fn rhcr_reset_clears_state() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig::auto(25, 1);
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let _ = solver.step(&ctx, &agents, &mut cache, &mut rng);

        solver.reset();
        // After reset, first call should replan again
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));
    }

    /// Regression test for the RHCR-PBS audit (Step 2 of solver-refocus).
    ///
    /// Locks the current lazy-priority PBS throughput on a known instance so a
    /// future change to the planner (e.g. switching to eager mode) is detected.
    /// The lower bound is set generously below the measured baseline so that
    /// minor implementation drift doesn't break the test, but a regression that
    /// drops PBS to all-Wait or all-fallback behavior would trip it.
    ///
    /// Measured baseline 2026-04-08 (post PAAMS 2026 RHCR-PBS fidelity port):
    /// tp = 0.435 tasks/tick on warehouse_large, 40 agents, random scheduler,
    /// 200 ticks. This is a 10× jump from the pre-port `0.040` baseline; the
    /// fix was the eager-mode + peek-chain + best-effort sequential-A* port
    /// (Streams B + C of the sprint). The floor is set to `0.20` — well above
    /// any reasonable noise band but conservative enough that a minor
    /// tuning change doesn't break CI.
    ///
    /// Reference: docs/papers_codes/rhcr/src/PBS.cpp (Jiaoyang-Li/RHCR).
    #[test]
    fn rhcr_pbs_throughput_regression() {
        use crate::core::topology::TopologyRegistry;
        use crate::experiment::config::ExperimentConfig;
        use crate::experiment::runner::run_single_experiment;

        // Validate the topology loads (defends against silent topology breakage)
        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        assert!(registry.find("warehouse_large").is_some(), "warehouse_large.json missing");

        let config = ExperimentConfig {
            solver_name: "rhcr_pbs".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 40,
            seed: 42,
            tick_count: 200,
            custom_map: None,
        };
        let result = run_single_experiment(&config);
        let tp = result.baseline_metrics.avg_throughput;
        eprintln!("rhcr_pbs_throughput_regression: tp={tp:.4} tasks/tick");
        // Floor: 0.20 — 46% of post-port baseline (0.435 measured 2026-04-08).
        // A drop below this would indicate a regression in `plan_agent`,
        // `find_consistent_paths`, or the best-partial sequential-A* fallback.
        // Higher throughput is fine.
        assert!(
            tp >= 0.20,
            "RHCR-PBS regression: avg_throughput {tp:.4} fell below the 0.20 \
             floor (baseline measured 2026-04-08: 0.435). Likely a regression \
             in eager-mode PBS, plan_agent sequential A*, or the best-partial \
             fallback. See src/solver/rhcr/pbs_planner.rs."
        );
    }
}
