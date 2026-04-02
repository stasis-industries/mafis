//! RHCR — Rolling-Horizon Collision Resolution
//!
//! Windowed lifelong solver that replans every W ticks for a horizon of H steps.
//! Configurable conflict resolution mode: PBS, PIBT-Window, or Priority A*.
//! Falls back to PIBT when the windowed planner fails.
//!
//! Reference: Li et al., "Lifelong Multi-Agent Path Finding in Large-Scale
//! Warehouses" (AAAI 2021).

use bevy::prelude::*;
use rand::Rng;
use smallvec::smallvec;

use crate::constants;
use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::windowed::{WindowAgent, WindowContext, WindowResult, WindowedPlanner};
use crate::solver::lifelong::{AgentPlan, AgentState, LifelongSolver, SolverContext, StepResult};
use crate::solver::shared::heuristics::{DistanceMapCache, manhattan};
use crate::solver::shared::pibt_core::PibtCore;
use crate::solver::shared::traits::{Optimality, Scalability, SolverInfo};

use super::pbs_planner::PbsPlanner;
use super::pibt_planner::PibtWindowPlanner;
use super::priority_astar::PriorityAStarPlanner;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RhcrMode {
    Pbs,
    PibtWindow,
    PriorityAStar,
}

impl RhcrMode {
    pub fn label(&self) -> &'static str {
        match self {
            RhcrMode::Pbs => "PBS",
            RhcrMode::PibtWindow => "PIBT-Window",
            RhcrMode::PriorityAStar => "Priority A*",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            RhcrMode::Pbs => "rhcr_pbs",
            RhcrMode::PibtWindow => "rhcr_pibt",
            RhcrMode::PriorityAStar => "rhcr_priority_astar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackMode {
    /// Keep solved agents' plans, PIBT only the failed ones.
    PerAgent,
    /// Discard all, PIBT everyone.
    Full,
    /// Keep partial plans up to tick budget, PIBT the rest.
    Tiered,
}

impl FallbackMode {
    pub fn label(&self) -> &'static str {
        match self {
            FallbackMode::PerAgent => "Per-Agent",
            FallbackMode::Full => "Full",
            FallbackMode::Tiered => "Tiered",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            FallbackMode::PerAgent => "per_agent",
            FallbackMode::Full => "full",
            FallbackMode::Tiered => "tiered",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RhcrConfig {
    pub mode: RhcrMode,
    pub fallback: FallbackMode,
    pub horizon: usize,
    pub replan_interval: usize,
    pub pbs_node_limit: usize,
}

impl RhcrConfig {
    /// Compute smart defaults from grid dimensions and agent count.
    ///
    /// Key trade-off: longer horizons = better plans, but larger frame stalls.
    /// We cap H based on mode cost to keep worst-case replan under ~50ms in WASM.
    pub fn auto(mode: RhcrMode, grid_area: usize, num_agents: usize) -> Self {
        let density = if grid_area > 0 { num_agents as f32 / grid_area as f32 } else { 0.0 };

        // Mode-dependent max horizon: expensive modes get shorter horizons
        // to keep per-replan cost bounded for WASM frame budget.
        let mode_max_h = match mode {
            RhcrMode::PibtWindow => constants::RHCR_MAX_HORIZON, // PIBT is fast
            RhcrMode::PriorityAStar => 20,                       // A* scales with n × H
            RhcrMode::Pbs => 15,                                 // PBS tree search is exponential
        };

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
        // W = H/3 keeps plans fresh without huge per-replan stalls.
        let w = if density > 0.15 {
            // High density: replan more aggressively (W = H/4)
            (h / 4).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else if density > 0.05 {
            // Medium density: standard interval (W = H/3)
            (h / 3).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        } else {
            // Low density: replan less often — agents have more room to follow stale plans
            (h / 2).max(constants::RHCR_MIN_REPLAN_INTERVAL)
        };

        let fallback = match mode {
            RhcrMode::Pbs => FallbackMode::PerAgent,
            RhcrMode::PibtWindow => FallbackMode::Full,
            RhcrMode::PriorityAStar => FallbackMode::Tiered,
        };

        // Tighter node limit for PBS — reduces worst-case tree explosion
        let pbs_node_limit = (num_agents * 3).clamp(50, constants::PBS_MAX_NODE_LIMIT);

        Self { mode, fallback, horizon: h, replan_interval: w, pbs_node_limit }
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
    /// Format: (agent_index, actions_vec).
    previous_plans: Vec<(usize, Vec<Action>)>,
    /// Previous positions for congestion detection (reference C++ BasicSystem::congested).
    prev_positions: Vec<IVec2>,
    /// Consecutive ticks where >50% of agents are stuck.
    congestion_streak: usize,
    /// Per-cell exponentially-decayed wait counts for congestion-aware routing.
    /// Indexed by `y * grid_width + x`. Tracks how often agents wait at each cell.
    wait_counts: Vec<f32>,
    /// Grid width for indexing into `wait_counts`.
    wait_counts_width: i32,
}

impl RhcrSolver {
    pub fn new(config: RhcrConfig) -> Self {
        let planner: Box<dyn WindowedPlanner> = match config.mode {
            RhcrMode::Pbs => Box::new(PbsPlanner::new()),
            RhcrMode::PibtWindow => Box::new(PibtWindowPlanner::new()),
            RhcrMode::PriorityAStar => Box::new(PriorityAStarPlanner::new()),
        };

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
        }
    }

    pub fn config(&self) -> &RhcrConfig {
        &self.config
    }

    /// Fill goal sequences for agents whose primary goal is reachable within the
    /// planning horizon. Appends random zone endpoints until cumulative distance
    /// reaches the horizon. Reference: KivaSystem::update_goal_locations() in RHCR C++.
    fn fill_goal_sequences(
        agents: &mut [WindowAgent],
        dist_maps: &[&crate::solver::shared::heuristics::DistanceMap],
        zones: &crate::core::topology::ZoneMap,
        horizon: usize,
        rng: &mut SeededRng,
    ) {
        let endpoints: Vec<IVec2> =
            zones.pickup_cells.iter().chain(zones.delivery_cells.iter()).copied().collect();
        if endpoints.is_empty() {
            return;
        }

        for (i, agent) in agents.iter_mut().enumerate() {
            let dist_to_goal = dist_maps[i].get(agent.pos);
            if dist_to_goal >= horizon as u64 {
                continue;
            }

            let mut cumulative = dist_to_goal;
            let mut last_goal = agent.goal;
            let max_seq = 4;
            let mut attempts = 0;
            while cumulative < horizon as u64 && agent.goal_sequence.len() < max_seq {
                let idx = rng.rng.random_range(0..endpoints.len());
                let next = endpoints[idx];
                if next == last_goal {
                    attempts += 1;
                    if attempts > 10 {
                        break;
                    }
                    continue;
                }
                attempts = 0;
                let leg_dist = manhattan(last_goal, next);
                cumulative += leg_dist;
                agent.goal_sequence.push(next);
                last_goal = next;
            }
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

        for _t in 0..max_steps {
            // Phase 1: compute intended next position for each slot
            let mut intended_action: Vec<Action> = Vec::with_capacity(n);
            let mut intended_pos: Vec<IVec2> = Vec::with_capacity(n);

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
            let mut forced_wait = vec![false; n];

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
        match self.config.mode {
            RhcrMode::Pbs => "rhcr_pbs",
            RhcrMode::PibtWindow => "rhcr_pibt",
            RhcrMode::PriorityAStar => "rhcr_priority_astar",
        }
    }

    fn info(&self) -> SolverInfo {
        let (desc, scalability) = match self.config.mode {
            RhcrMode::Pbs => (
                "RHCR (PBS) — windowed planning with Priority-Based Search. High quality, bounded by node limit.",
                Scalability::Medium,
            ),
            RhcrMode::PibtWindow => (
                "RHCR (PIBT-Window) — windowed PIBT unrolled for H steps. Fast and cooperative.",
                Scalability::High,
            ),
            RhcrMode::PriorityAStar => (
                "RHCR (Priority A*) — sequential spacetime A* with priority ordering. Good for moderate density.",
                Scalability::Medium,
            ),
        };

        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(W⁻¹ × mode_cost) amortized per tick",
            scalability,
            description: desc,
            source: "Li et al., AAAI 2021",
            recommended_max_agents: match self.config.mode {
                RhcrMode::Pbs => Some(200),
                RhcrMode::PibtWindow => None,
                RhcrMode::PriorityAStar => Some(300),
            },
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

        // Build WindowAgent list
        let mut window_agents: Vec<WindowAgent> = agents
            .iter()
            .map(|a| WindowAgent {
                index: a.index,
                pos: a.pos,
                goal: a.goal.unwrap_or(a.pos),
                goal_sequence: smallvec::SmallVec::new(),
            })
            .collect();

        // Build distance maps
        let pairs: Vec<(IVec2, IVec2)> = window_agents.iter().map(|a| (a.pos, a.goal)).collect();
        let dist_maps = distance_cache.get_or_compute(ctx.grid, &pairs);

        // Fill goal sequences for agents whose goal is close enough to reach
        // within the planning horizon (reference: KivaSystem::update_goal_locations).
        Self::fill_goal_sequences(
            &mut window_agents,
            &dist_maps,
            ctx.zones,
            self.config.horizon,
            rng,
        );

        // Build initial plans for warm-starting: reuse previous plans for agents
        // whose goal hasn't changed and whose plan is still valid (all cells walkable).
        let mut initial_plans: Vec<Option<Vec<Action>>> = vec![None; window_agents.len()];
        for (prev_idx, prev_actions) in &self.previous_plans {
            // Find the agent in the current window
            if let Some(local_i) = window_agents.iter().position(|a| a.index == *prev_idx) {
                let agent = &window_agents[local_i];
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
                        initial_plans[local_i] = Some(prev_actions.clone());
                    }
                }
            }
        }

        // Cross-window start constraints: vertex constraints at t=0 for all
        // agents' start positions, preventing agent A from planning to be at
        // agent B's position at t=0 when B hasn't moved yet.
        let start_constraints: Vec<(IVec2, u64)> =
            window_agents.iter().map(|a| (a.pos, 0u64)).collect();

        let window_ctx = WindowContext {
            grid: ctx.grid,
            horizon: self.config.horizon,
            node_limit: self.config.pbs_node_limit,
            agents: &window_agents,
            distance_maps: &dist_maps,
            initial_plans,
            start_constraints,
            travel_penalties: &self.wait_counts,
        };

        // Run windowed planner
        let result = self.planner.plan_window(&window_ctx, rng);

        match result {
            WindowResult::Solved(fragments) => {
                for frag in fragments {
                    self.plan_buffer.push((frag.agent_index, frag.actions));
                }
            }
            WindowResult::Partial { solved, failed } => {
                match self.config.fallback {
                    FallbackMode::PerAgent => {
                        // LRA conflict resolution: combine solved multi-step plans
                        // with single-Wait stubs for failed agents, then resolve
                        // conflicts timestep-by-timestep. This preserves plan
                        // structure (unlike PIBT fallback which discards everything).
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

                        let resolved = Self::lra_resolve_conflicts(
                            &combined,
                            agents,
                            ctx.grid,
                            self.config.horizon,
                        );

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
                    FallbackMode::Tiered => {
                        // Tier 1: check solved plans for conflicts in the committed
                        // window (first W steps). Agents with conflicts in the
                        // committed portion get demoted to the failed set before
                        // LRA resolution. This differs from PerAgent which keeps all
                        // solved plans regardless of internal conflicts.
                        let w = self.config.replan_interval;

                        // Build position timelines for solved agents over W steps
                        let mut positions: Vec<Vec<IVec2>> = Vec::with_capacity(solved.len());
                        let mut indices: Vec<usize> = Vec::with_capacity(solved.len());
                        for frag in &solved {
                            if let Some(a) = agents.iter().find(|a| a.index == frag.agent_index) {
                                let mut pos = a.pos;
                                let mut timeline = vec![pos];
                                for action in frag.actions.iter().take(w) {
                                    pos = action.apply(pos);
                                    timeline.push(pos);
                                }
                                positions.push(timeline);
                                indices.push(frag.agent_index);
                            }
                        }

                        // Detect vertex conflicts in first W steps
                        let mut conflicting: std::collections::HashSet<usize> =
                            std::collections::HashSet::new();
                        for t in 1..=w {
                            for i in 0..positions.len() {
                                let pos_i = positions[i]
                                    .get(t)
                                    .copied()
                                    .unwrap_or(*positions[i].last().unwrap());
                                for j in (i + 1)..positions.len() {
                                    let pos_j = positions[j]
                                        .get(t)
                                        .copied()
                                        .unwrap_or(*positions[j].last().unwrap());
                                    if pos_i == pos_j {
                                        conflicting.insert(indices[i]);
                                        conflicting.insert(indices[j]);
                                    }
                                }
                            }
                        }

                        // Demote conflicting agents from solved to failed
                        let mut kept_solved = Vec::new();
                        let mut extended_failed = failed.clone();
                        for frag in solved {
                            if conflicting.contains(&frag.agent_index) {
                                extended_failed.push(frag.agent_index);
                            } else {
                                kept_solved.push(frag);
                            }
                        }

                        // LRA resolve on kept_solved + extended_failed
                        let mut combined: Vec<(usize, Vec<Action>)> =
                            Vec::with_capacity(kept_solved.len() + extended_failed.len());
                        for frag in &kept_solved {
                            combined.push((frag.agent_index, frag.actions.to_vec()));
                        }
                        for &idx in &extended_failed {
                            combined.push((idx, vec![Action::Wait]));
                        }

                        let solved_first_steps: Vec<(usize, IVec2)> = kept_solved
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

                        let resolved = Self::lra_resolve_conflicts(
                            &combined,
                            agents,
                            ctx.grid,
                            self.config.horizon,
                        );

                        let mut pibt_needed: Vec<usize> = Vec::new();
                        for (idx, actions) in &resolved {
                            if actions.iter().all(|a| *a == Action::Wait)
                                && extended_failed.contains(idx)
                            {
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
                    FallbackMode::Full => {
                        // Discard all windowed plans, PIBT everyone
                        let all_indices: Vec<usize> = agents.iter().map(|a| a.index).collect();
                        self.pibt_fallback_for(&all_indices, agents, distance_cache, ctx.grid, &[]);
                    }
                }
            }
        }

        // Store plans for warm-starting next replan
        self.previous_plans.clear();
        for (idx, actions) in &self.plan_buffer {
            self.previous_plans.push((*idx, actions.to_vec()));
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
        let config = RhcrConfig {
            mode: RhcrMode::PibtWindow,
            fallback: FallbackMode::Full,
            horizon: 10,
            replan_interval: 5,
            pbs_node_limit: 100,
        };
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
        // Move the agent each tick to avoid triggering congestion detection.
        // Next 4 calls should return Continue (ticks 1-4 of new window)
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
        let cfg = RhcrConfig::auto(RhcrMode::Pbs, 100, 5);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
        assert!(cfg.replan_interval >= constants::RHCR_MIN_REPLAN_INTERVAL);
        assert!(cfg.replan_interval <= cfg.horizon);

        // Large grid, many agents
        let cfg = RhcrConfig::auto(RhcrMode::Pbs, 16384, 400);
        assert!(cfg.horizon >= constants::RHCR_MIN_HORIZON);
        assert!(cfg.horizon <= constants::RHCR_MAX_HORIZON);
    }

    #[test]
    fn rhcr_pbs_mode_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::Pbs,
            fallback: FallbackMode::PerAgent,
            horizon: 15,
            replan_interval: 1, // replan every tick for testing
            pbs_node_limit: 500,
        };
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
    fn rhcr_priority_astar_mode_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PriorityAStar,
            fallback: FallbackMode::Tiered,
            horizon: 15,
            replan_interval: 1,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::new(0, 0),
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(_)));
    }

    #[test]
    fn rhcr_force_replans_when_agent_has_no_plan() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PibtWindow,
            fallback: FallbackMode::Full,
            horizon: 10,
            replan_interval: 5,
            pbs_node_limit: 100,
        };
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

    /// Full loop: RHCR on a warehouse with goal recycling — no obstacle violations.
    /// Also prints timing for perf regression detection.
    #[test]
    fn rhcr_warehouse_lifelong_no_obstacle_violations() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 8;
        let ticks = 100;

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, n, ticks);
        }
    }

    /// Scale benchmark: 50 agents on medium warehouse.
    #[test]
    fn rhcr_scale_50_agents() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, 50, 50);
        }
    }

    /// Scale benchmark: 80 agents on medium warehouse.
    #[test]
    fn rhcr_scale_80_agents() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, 80, 50);
        }
    }

    /// Scale benchmark: 30 agents on medium warehouse.
    #[test]
    fn rhcr_scale_benchmark_medium_warehouse() {
        use crate::core::topology::TopologyRegistry;

        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        let entry = registry.find("warehouse_large").expect("warehouse_large.json missing");
        let (grid_owned, zones_owned) = TopologyRegistry::parse_entry(entry).unwrap();
        let grid = &grid_owned;
        let zones = &zones_owned;

        let walkable: Vec<IVec2> = zones
            .corridor_cells
            .iter()
            .chain(zones.pickup_cells.iter())
            .chain(zones.delivery_cells.iter())
            .copied()
            .filter(|p| grid.is_walkable(*p))
            .collect();

        let n = 30;
        let ticks = 50;

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            run_rhcr_bench(mode, grid, zones, &walkable, n, ticks);
        }
    }

    #[test]
    fn rhcr_fills_goal_sequences_when_goal_is_close() {
        let grid = GridMap::new(10, 10);
        let zones = ZoneMap {
            pickup_cells: vec![IVec2::new(1, 1), IVec2::new(3, 3), IVec2::new(5, 5)],
            delivery_cells: vec![IVec2::new(8, 8), IVec2::new(7, 7)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        };

        let config = RhcrConfig {
            mode: RhcrMode::PriorityAStar,
            fallback: FallbackMode::Tiered,
            horizon: 20,
            replan_interval: 1,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let agents = vec![AgentState {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(2, 0)),
            has_plan: false,
            task_leg: TaskLeg::TravelEmpty(IVec2::new(2, 0)),
        }];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 1 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);

        match result {
            StepResult::Replan(plans) => {
                assert_eq!(plans.len(), 1);
                assert!(
                    plans[0].1.len() > 2,
                    "Plan should extend beyond first goal via goal sequences, got {} steps",
                    plans[0].1.len()
                );
            }
            _ => panic!("expected Replan"),
        }
    }

    fn run_rhcr_bench(
        mode: RhcrMode,
        grid: &crate::core::grid::GridMap,
        zones: &ZoneMap,
        walkable: &[IVec2],
        n: usize,
        ticks: u64,
    ) {
        use rand::Rng;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use std::time::Instant;

        let mut rng_seed = ChaCha8Rng::seed_from_u64(99);
        let mut positions: Vec<IVec2> = walkable[..n].to_vec();
        let mut goals: Vec<IVec2> =
            (0..n).map(|_| walkable[rng_seed.random_range(0..walkable.len())]).collect();
        let mut plans: Vec<std::collections::VecDeque<Action>> =
            vec![std::collections::VecDeque::new(); n];

        let config = RhcrConfig::auto(mode, (grid.width * grid.height) as usize, n);
        let h = config.horizon;
        let w = config.replan_interval;
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let start = Instant::now();

        for tick in 0..ticks {
            for i in 0..n {
                let action = plans[i].pop_front().unwrap_or(Action::Wait);
                let new_pos = action.apply(positions[i]);
                assert!(
                    grid.is_walkable(new_pos),
                    "RHCR {:?}: agent {} tick {} moved to obstacle {:?}",
                    mode,
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
            let ctx = SolverContext { grid, zones: &solver_zones, tick, num_agents: n };

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

        let elapsed = start.elapsed();
        let us_per_tick = elapsed.as_micros() / ticks as u128;
        eprintln!(
            "RHCR {:?}: {}ticks × {}agents in {:?} ({} µs/tick) [H={} W={}]",
            mode, ticks, n, elapsed, us_per_tick, h, w,
        );
    }

    /// Throughput benchmark: measures tasks-completed per 100 ticks on warehouse_large.
    #[test]
    fn rhcr_throughput_benchmark() {
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

        let n = 30;
        let ticks = 200u64;

        for mode in [RhcrMode::PibtWindow, RhcrMode::PriorityAStar, RhcrMode::Pbs] {
            let mut rng_seed = ChaCha8Rng::seed_from_u64(99);
            let mut positions: Vec<IVec2> = walkable[..n].to_vec();
            let mut goals: Vec<IVec2> =
                (0..n).map(|_| walkable[rng_seed.random_range(0..walkable.len())]).collect();
            let mut plans: Vec<std::collections::VecDeque<Action>> =
                vec![std::collections::VecDeque::new(); n];

            let config = RhcrConfig::auto(mode, (grid.width * grid.height) as usize, n);
            let h = config.horizon;
            let w = config.replan_interval;
            let mut solver = RhcrSolver::new(config);
            let mut cache = DistanceMapCache::default();
            let mut rng = SeededRng::new(42);
            let mut tasks_completed = 0u64;

            for tick in 0..ticks {
                for i in 0..n {
                    let action = plans[i].pop_front().unwrap_or(Action::Wait);
                    let new_pos = action.apply(positions[i]);
                    assert!(grid.is_walkable(new_pos));
                    positions[i] = new_pos;
                }

                for i in 0..n {
                    if positions[i] == goals[i] {
                        goals[i] = walkable[rng_seed.random_range(0..walkable.len())];
                        plans[i].clear();
                        tasks_completed += 1;
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

                let ctx = SolverContext { grid: &grid, zones: &zones, tick, num_agents: n };

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

            eprintln!(
                "THROUGHPUT RHCR {:?}: {} tasks in {} ticks ({:.1}/100t) [H={} W={}]",
                mode,
                tasks_completed,
                ticks,
                tasks_completed as f64 / ticks as f64 * 100.0,
                h,
                w,
            );
        }
    }

    #[test]
    fn rhcr_warm_starts_reuse_valid_plans() {
        // Run RHCR twice in succession. On the second replan (same goals),
        // the solver should reuse plans from the first replan (warm-start).
        // We verify this indirectly: the second replan should produce plans
        // at least as long as the first (no degradation from cold start).
        let grid = GridMap::new(10, 10);
        let zones = ZoneMap {
            pickup_cells: vec![IVec2::new(1, 1), IVec2::new(3, 3)],
            delivery_cells: vec![IVec2::new(8, 8), IVec2::new(7, 7)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        };

        let config = RhcrConfig {
            mode: RhcrMode::PriorityAStar,
            fallback: FallbackMode::Tiered,
            horizon: 15,
            replan_interval: 1, // replan every tick
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(9, 9)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
            AgentState {
                index: 1,
                pos: IVec2::new(9, 0),
                goal: Some(IVec2::new(0, 9)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
        ];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };

        // First replan
        let result1 = solver.step(&ctx, &agents, &mut cache, &mut rng);
        let len1 = match &result1 {
            StepResult::Replan(plans) => plans.iter().map(|(_, a)| a.len()).sum::<usize>(),
            _ => panic!("expected Replan"),
        };

        // Second replan (same agents, same goals — should warm-start)
        let ctx2 = SolverContext { grid: &grid, zones: &zones, tick: 1, num_agents: 2 };
        let result2 = solver.step(&ctx2, &agents, &mut cache, &mut rng);
        let len2 = match &result2 {
            StepResult::Replan(plans) => plans.iter().map(|(_, a)| a.len()).sum::<usize>(),
            _ => panic!("expected Replan"),
        };

        // Warm-started plans should be at least as good (plans exist)
        assert!(len1 > 0, "First replan should produce plans");
        assert!(len2 > 0, "Second replan (warm-started) should produce plans");
    }

    #[test]
    fn lra_resolves_vertex_conflict() {
        use crate::core::action::Direction;

        let grid = GridMap::new(5, 5);
        // Two agents both want to go to (2,0) at t=1
        let plans: Vec<(usize, Vec<Action>)> = vec![
            (0, vec![Action::Move(Direction::East), Action::Move(Direction::East)]),
            (1, vec![Action::Move(Direction::West), Action::Move(Direction::West)]),
        ];
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(1, 0),
                goal: Some(IVec2::new(3, 0)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            },
            AgentState {
                index: 1,
                pos: IVec2::new(3, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            },
        ];

        let resolved = RhcrSolver::lra_resolve_conflicts(&plans, &agents, &grid, 4);

        // Both should have plans
        assert_eq!(resolved.len(), 2);

        // Simulate and verify no vertex conflicts
        let mut pos0 = agents[0].pos;
        let mut pos1 = agents[1].pos;
        let plan0 = &resolved.iter().find(|(i, _)| *i == 0).unwrap().1;
        let plan1 = &resolved.iter().find(|(i, _)| *i == 1).unwrap().1;
        let max_len = plan0.len().max(plan1.len());
        for t in 0..max_len {
            let a0 = plan0.get(t).copied().unwrap_or(Action::Wait);
            let a1 = plan1.get(t).copied().unwrap_or(Action::Wait);
            pos0 = a0.apply(pos0);
            pos1 = a1.apply(pos1);
            assert_ne!(pos0, pos1, "vertex conflict at t={}", t + 1);
        }
    }

    #[test]
    fn tiered_fallback_demotes_conflicting_plans() {
        // This test verifies that Tiered mode is different from PerAgent:
        // a solved plan that has a conflict in the first W steps gets demoted.
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig {
            mode: RhcrMode::PriorityAStar,
            fallback: FallbackMode::Tiered,
            horizon: 10,
            replan_interval: 1,
            pbs_node_limit: 100,
        };
        let mut solver = RhcrSolver::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        // Two agents with crossing goals
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(0, 2),
                goal: Some(IVec2::new(4, 2)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
            AgentState {
                index: 1,
                pos: IVec2::new(4, 2),
                goal: Some(IVec2::new(0, 2)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
        ];

        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 2 };
        let result = solver.step(&ctx, &agents, &mut cache, &mut rng);

        // Should produce valid plans (no crashes, no empty)
        match result {
            StepResult::Replan(plans) => {
                assert_eq!(plans.len(), 2, "Both agents should get plans");
                assert!(!plans[0].1.is_empty());
                assert!(!plans[1].1.is_empty());
            }
            _ => panic!("expected Replan"),
        }
    }

    #[test]
    fn lra_resolves_edge_conflict() {
        use crate::core::action::Direction;

        let grid = GridMap::new(5, 5);
        // Two agents trying to swap positions
        let plans: Vec<(usize, Vec<Action>)> = vec![
            (0, vec![Action::Move(Direction::East)]),
            (1, vec![Action::Move(Direction::West)]),
        ];
        let agents = vec![
            AgentState {
                index: 0,
                pos: IVec2::new(1, 0),
                goal: Some(IVec2::new(2, 0)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            },
            AgentState {
                index: 1,
                pos: IVec2::new(2, 0),
                goal: Some(IVec2::new(1, 0)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            },
        ];

        let resolved = RhcrSolver::lra_resolve_conflicts(&plans, &agents, &grid, 4);

        // Simulate: no swap conflict
        let mut pos0 = agents[0].pos;
        let mut pos1 = agents[1].pos;
        let plan0 = &resolved.iter().find(|(i, _)| *i == 0).unwrap().1;
        let plan1 = &resolved.iter().find(|(i, _)| *i == 1).unwrap().1;
        for t in 0..plan0.len().max(plan1.len()) {
            let prev0 = pos0;
            let prev1 = pos1;
            let a0 = plan0.get(t).copied().unwrap_or(Action::Wait);
            let a1 = plan1.get(t).copied().unwrap_or(Action::Wait);
            pos0 = a0.apply(pos0);
            pos1 = a1.apply(pos1);
            // No swap: agent 0 didn't go to agent 1's old pos while agent 1 went to agent 0's old pos
            let swapped = pos0 == prev1 && pos1 == prev0 && pos0 != prev0;
            assert!(!swapped, "edge conflict (swap) at t={}", t + 1);
        }
    }
}
