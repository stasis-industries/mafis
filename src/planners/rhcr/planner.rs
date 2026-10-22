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
//! Fidelity notes (reference file:line in Jiaoyang-Li/RHCR):
//! - Cadence: full-fleet PBS every `simulation_window` ticks with a
//!   `planning_window` lookahead (driver.cpp:116-119, KivaSystem.cpp:209).
//!   Between windows only agents left without a plan are planned, around
//!   the others' fixed paths (`new_agents` vs `initial_rt`,
//!   BasicSystem.cpp:604-637), counted in the `extra_replans` telemetry.
//! - Goals: MAFIS reveals an agent's next goal only when it arrives, so agents
//!   plan to their current goal and hold it — the reference's
//!   `hold_endpoints` mode (KivaSystem.cpp:74), not its default chain of
//!   pre-generated future goals.
//! - PBS failure: the best node's conflicting paths go through LRA conflict
//!   resolution (BasicSystem.cpp:663-673). Agents LRA leaves fully waiting get
//!   one PIBT step — a MAFIS addition.
//! - Congestion: `BasicSystem::congested()` (BasicSystem.cpp:310) is
//!   evaluated every window and counted; the reference stops the run there.
//!
//! See `pbs_planner.rs` for inner-planner deviations from `PBS.cpp`.

use bevy::prelude::*;
use smallvec::smallvec;

use crate::constants;
use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::windowed::{WindowAgent, WindowContext, WindowResult, WindowedPlanner};
use crate::planners::toolkit::bytes::{f32s_from_bytes, f32s_to_bytes};
use crate::planners::toolkit::pibt_core::PibtCore;
use crate::planning::heuristics::DistanceMapCache;
use crate::planning::params::{ParamKind, ParamSpec, ResolvedParams};
use crate::planning::planner::{AgentPlan, AgentView, Decision, Observation, Planner, Telemetry};

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

/// Parameters RHCR-PBS declares. Defaults are the reference values: replan
/// every `simulation_window` = 5 ticks with a `planning_window` of 10 (RHCR
/// README example; driver.cpp:116-119), independent of the grid.
pub const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        key: "horizon",
        label: "Planning window (H)",
        help: "Ticks over which PBS resolves conflicts.",
        kind: ParamKind::Int {
            min: constants::RHCR_MIN_HORIZON as i64,
            max: constants::RHCR_MAX_HORIZON as i64,
            default: constants::RHCR_PLANNING_HORIZON_DEFAULT as i64,
        },
    },
    ParamSpec {
        key: "replan_interval",
        label: "Replan interval (W)",
        help: "Ticks between planning windows.",
        kind: ParamKind::Int {
            min: constants::RHCR_MIN_REPLAN_INTERVAL as i64,
            max: constants::RHCR_MAX_REPLAN_INTERVAL as i64,
            default: constants::RHCR_SIMULATION_WINDOW_DEFAULT as i64,
        },
    },
    ParamSpec {
        key: "nodes_per_agent",
        label: "PBS nodes per agent",
        help: "PBS node budget per agent; the fleet's total is kept within the node-limit bounds.",
        kind: ParamKind::Int {
            min: 1,
            max: constants::PBS_MAX_NODE_LIMIT as i64,
            default: constants::PBS_NODES_PER_AGENT as i64,
        },
    },
];

impl RhcrConfig {
    /// The configuration for `fleet` agents. The PBS budget scales with the
    /// fleet (see `PBS_NODES_PER_AGENT`).
    pub fn from_params(params: &ResolvedParams, fleet: usize) -> Self {
        Self {
            horizon: params.int("horizon") as usize,
            replan_interval: params.int("replan_interval") as usize,
            pbs_node_limit: (fleet * params.int("nodes_per_agent") as usize)
                .clamp(constants::PBS_MIN_NODE_LIMIT, constants::PBS_MAX_NODE_LIMIT),
        }
    }
}

// ---------------------------------------------------------------------------
// RHCR Solver
// ---------------------------------------------------------------------------

pub struct RhcrPlanner {
    config: RhcrConfig,
    planner: Box<dyn WindowedPlanner>,
    pibt_fallback: PibtCore,
    ticks_since_replan: usize,
    plan_buffer: Vec<AgentPlan>,
    /// Last plan issued per agent id and the tick it was issued; the
    /// unexecuted rest is the agent's current path.
    issued: Vec<(u64, smallvec::SmallVec<[Action; 20]>)>,
    /// Count of between-window plans for agents left without a plan.
    extra_replans: usize,
    /// Last seen position per agent id, for the reference congestion test.
    last_pos: Vec<Option<IVec2>>,
    /// Whether each agent id moved at least once in the current window.
    moved_in_window: Vec<bool>,
    /// Windows checked / windows where more than half the agents never moved
    /// (reference `BasicSystem::congested`, BasicSystem.cpp:310). The
    /// reference stops the run there; MAFIS only counts it.
    congestion_checks: usize,
    congested_windows: usize,
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
    scratch_fixed_vertices: Vec<(IVec2, u64)>,
    scratch_fixed_edges: Vec<(IVec2, IVec2, u64)>,
    /// Total number of times `plan_window` was invoked during this run.
    /// Incremented before each call; reset on `reset()`.
    pbs_total_replans: usize,
    /// Count of windows that returned `WindowResult::Partial` (fell into
    /// LRA + PIBT fallback). Used to compute `pbs_partial_rate` as an
    /// observatory probe for PBS scalability cliffs.
    pbs_partial_count: usize,
}

impl RhcrPlanner {
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
            issued: Vec::new(),
            extra_replans: 0,
            last_pos: Vec::new(),
            moved_in_window: Vec::new(),
            congestion_checks: 0,
            congested_windows: 0,
            scratch_window_agents: Vec::new(),
            scratch_initial_plans: Vec::new(),
            scratch_start_constraints: Vec::new(),
            scratch_fixed_vertices: Vec::new(),
            scratch_fixed_edges: Vec::new(),
            pbs_total_replans: 0,
            pbs_partial_count: 0,
        }
    }

    pub fn config(&self) -> &RhcrConfig {
        &self.config
    }

    /// Track per-agent movement each tick for the reference congestion test.
    fn track_movement(&mut self, agents: &[AgentView]) {
        for a in agents {
            if self.last_pos.len() <= a.index {
                self.last_pos.resize(a.index + 1, None);
                self.moved_in_window.resize(a.index + 1, false);
            }
            if self.last_pos[a.index].is_some_and(|p| p != a.pos) {
                self.moved_in_window[a.index] = true;
            }
            self.last_pos[a.index] = Some(a.pos);
        }
    }

    /// Reference `BasicSystem::congested()` (BasicSystem.cpp:310): more than
    /// half the agents made no move during the window that just ended.
    /// Called at each replan; the first window has no history and is skipped.
    fn close_window(&mut self, agents: &[AgentView]) {
        if self.pbs_total_replans > 0 && !agents.is_empty() {
            let still = agents.iter().filter(|a| !self.moved_in_window[a.index]).count();
            self.congestion_checks += 1;
            if still * 2 > agents.len() {
                self.congested_windows += 1;
            }
        }
        self.moved_in_window.fill(false);
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
        agents: &[AgentView],
        _grid: &GridMap,
        max_steps: usize,
    ) -> Vec<AgentPlan> {
        use smallvec::SmallVec;

        let n = plans.len();
        if n == 0 {
            return Vec::new();
        }

        // Current position per slot (initialized from AgentView)
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
        agents: &[AgentView],
        distance_cache: &mut DistanceMapCache,
        grid: &GridMap,
        solved_first_steps: &[(usize, IVec2)],
    ) {
        if failed_indices.is_empty() {
            return;
        }

        // Build sub-problem for failed agents only
        let sub_agents: Vec<&AgentView> = failed_indices
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
        let ids: Vec<usize> = sub_agents
            .iter()
            .map(|a| a.index)
            .chain(solved_first_steps.iter().map(|&(idx, _)| idx))
            .collect();
        let actions = self.pibt_fallback.one_step_with_tasks(
            &all_positions,
            &all_goals,
            grid,
            &all_dist_maps,
            &ids,
            &[],
        );

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

/// Agent has somewhere to go but no plan (new goal, cleared by the runner).
fn needs_plan(a: &AgentView) -> bool {
    !a.has_plan && a.goal.is_some_and(|g| g != a.pos)
}

/// The unexecuted rest of an agent's last issued plan (empty if the runner
/// cleared it). Plans issued at tick `t` start executing at `t + 1`.
fn remaining_plan<'p>(
    issued: &'p [(u64, smallvec::SmallVec<[Action; 20]>)],
    a: &AgentView,
    tick: u64,
) -> &'p [Action] {
    match issued.get(a.index) {
        Some((t, plan)) if a.has_plan => &plan[((tick - t) as usize).min(plan.len())..],
        _ => &[],
    }
}

/// Hard constraints for a path that stays fixed: the cells it visits, its
/// moves (no swaps), and its last cell held to the horizon.
fn fixed_path_constraints(
    start: IVec2,
    plan: &[Action],
    horizon: usize,
    vertices: &mut Vec<(IVec2, u64)>,
    edges: &mut Vec<(IVec2, IVec2, u64)>,
) {
    let mut pos = start;
    for (t, action) in plan.iter().take(horizon).enumerate() {
        let next = action.apply(pos);
        vertices.push((next, t as u64 + 1));
        edges.push((next, pos, t as u64));
        pos = next;
    }
    for t in plan.len().min(horizon) + 1..=horizon {
        vertices.push((pos, t as u64));
    }
}

impl Planner for RhcrPlanner {
    fn reset(&mut self) {
        self.ticks_since_replan = self.config.replan_interval.saturating_sub(1);
        self.pibt_fallback.reset();
        self.planner.reset();
        self.plan_buffer.clear();
        self.issued.clear();
        self.extra_replans = 0;
        self.last_pos.clear();
        self.moved_in_window.clear();
        self.congestion_checks = 0;
        self.congested_windows = 0;
        self.scratch_window_agents.clear();
        self.scratch_initial_plans.clear();
        self.scratch_start_constraints.clear();
        self.pbs_total_replans = 0;
        self.pbs_partial_count = 0;
    }

    /// The fallback PIBT's and the windowed planner's priorities, as
    /// `[fallback length: u32 LE][fallback f32s][windowed-planner f32s]`.
    /// The window phase is not saved: a restored run replans at its next step
    /// instead of finishing the window it was in.
    fn snapshot(&self) -> Option<Vec<u8>> {
        let fallback = self.pibt_fallback.priorities();
        let mut bytes = (fallback.len() as u32).to_le_bytes().to_vec();
        bytes.extend(f32s_to_bytes(fallback));
        bytes.extend(f32s_to_bytes(&self.planner.save_priorities()));
        Some(bytes)
    }

    fn restore(&mut self, snapshot: &[u8]) {
        let Some((fallback_len, rest)) = snapshot.split_first_chunk::<4>() else {
            return;
        };
        let fallback_len = u32::from_le_bytes(*fallback_len) as usize;
        let priorities = f32s_from_bytes(rest);
        if priorities.len() >= fallback_len {
            self.pibt_fallback.set_priorities(&priorities[..fallback_len]);
            self.planner.restore_priorities(&priorities[fallback_len..]);
        }
    }

    fn step<'a>(
        &'a mut self,
        obs: &Observation,
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> Decision<'a> {
        let agents = obs.agents;
        self.ticks_since_replan += 1;

        // Sync PIBT fallback shuffle seed with tick for deterministic tie-breaking after rewind.
        self.pibt_fallback.set_shuffle_seed(obs.tick);
        self.pibt_fallback.set_tie_salt(rng.seed());

        self.track_movement(agents);

        // Full-fleet PBS every `simulation_window` ticks (KivaSystem.cpp:209).
        // In between, only agents left without a plan (new goal, runner-forced
        // wait, fault) are planned, around everyone else's unchanged paths —
        // the reference's `new_agents` planning against `initial_rt`
        // (BasicSystem.cpp:604-637), run on demand because MAFIS reveals goals
        // on arrival instead of pre-generating them.
        let window = self.ticks_since_replan >= self.config.replan_interval;
        self.plan_buffer.clear();
        if window {
            self.ticks_since_replan = 0;
            self.close_window(agents);
        } else if !agents.iter().any(needs_plan) {
            return Decision::Keep;
        } else {
            self.extra_replans += 1;
        }

        if agents.is_empty() {
            return Decision::Plans(&self.plan_buffer);
        }

        // Each agent plans to its current goal and holds it — the reference's
        // `hold_endpoints` mode (KivaSystem.cpp:74) — since its next goal is
        // unknown until it arrives. Agents outside this window keep their
        // remaining path, which becomes a hard constraint.
        self.scratch_window_agents.clear();
        self.scratch_initial_plans.clear();
        self.scratch_fixed_vertices.clear();
        self.scratch_fixed_edges.clear();
        for a in agents {
            let remaining = remaining_plan(&self.issued, a, obs.tick);
            if window || needs_plan(a) {
                self.scratch_window_agents.push(WindowAgent {
                    index: a.index,
                    pos: a.pos,
                    goal: a.goal.unwrap_or(a.pos),
                    goal_sequence: smallvec::SmallVec::new(),
                });
                // Warm start: the unexecuted rest of the last plan, if still walkable.
                let mut pos = a.pos;
                let valid = !remaining.is_empty()
                    && remaining.iter().all(|act| {
                        pos = act.apply(pos);
                        obs.grid.is_walkable(pos)
                    });
                self.scratch_initial_plans.push(valid.then(|| remaining.to_vec()));
            } else {
                fixed_path_constraints(
                    a.pos,
                    remaining,
                    self.config.horizon,
                    &mut self.scratch_fixed_vertices,
                    &mut self.scratch_fixed_edges,
                );
            }
        }

        self.scratch_start_constraints.clear();
        self.scratch_start_constraints
            .extend(self.scratch_window_agents.iter().map(|a| (a.pos, 0u64)));

        let window_ctx = WindowContext {
            grid: obs.grid,
            horizon: self.config.horizon,
            node_limit: self.config.pbs_node_limit,
            agents: &self.scratch_window_agents,
            distance_maps: &[],
            initial_plans: &self.scratch_initial_plans,
            start_constraints: &self.scratch_start_constraints,
            fixed_vertices: &self.scratch_fixed_vertices,
            fixed_edges: &self.scratch_fixed_edges,
        };

        self.pbs_total_replans += 1;
        let result = self.planner.plan_window(&window_ctx, distance_cache, rng);

        match result {
            WindowResult::Solved(fragments) => {
                for frag in fragments {
                    self.plan_buffer.push((frag.agent_index, frag.actions));
                }
            }
            WindowResult::Partial { solved, failed } => {
                self.pbs_partial_count += 1;
                // Reference behaviour on PBS failure: LRA resolves the best
                // node's remaining conflicts by inserting waits.
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
                    Self::lra_resolve_conflicts(&combined, agents, obs.grid, self.config.horizon);

                // LRA may leave a failed agent waiting the whole window; give
                // it one PIBT step (MAFIS addition).
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
                        obs.grid,
                        &solved_first_steps,
                    );
                }
            }
        }

        for (idx, actions) in &self.plan_buffer {
            if self.issued.len() <= *idx {
                self.issued.resize(*idx + 1, (0, smallvec::SmallVec::new()));
            }
            self.issued[*idx] = (obs.tick, actions.clone());
        }

        Decision::Plans(&self.plan_buffer)
    }

    /// - `pbs_partial_rate`: fraction of replan windows that could not close
    ///   the full fleet plan within the node budget (`WindowResult::Partial`),
    ///   a probe for PBS scalability cliffs; once a window ran.
    /// - `congested_window_rate`: fraction of windows in which more than half
    ///   the agents never moved (reference `BasicSystem::congested`, where the
    ///   reference run would stop); once a window was checked.
    /// - `extra_replans`: plans made between windows for agents left without
    ///   a plan (new goal, runner-forced wait, fault).
    fn telemetry(&self) -> Telemetry {
        let mut telemetry = Vec::new();
        if self.pbs_total_replans > 0 {
            let rate = self.pbs_partial_count as f64 / self.pbs_total_replans as f64;
            telemetry.push(("pbs_partial_rate".to_string(), rate));
        }
        if self.congestion_checks > 0 {
            let rate = self.congested_windows as f64 / self.congestion_checks as f64;
            telemetry.push(("congested_window_rate".to_string(), rate));
        }
        telemetry.push(("extra_replans".to_string(), self.extra_replans as f64));
        telemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::params::{ParamValue, ParamValues, resolve};

    fn telemetry_value(planner: &RhcrPlanner, key: &str) -> Option<f64> {
        planner.telemetry().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn config_for(values: &ParamValues, fleet: usize) -> RhcrConfig {
        RhcrConfig::from_params(&resolve(PARAMS, values).unwrap(), fleet)
    }

    #[test]
    fn default_params_match_reference_values() {
        let small = config_for(&ParamValues::default(), 12);
        assert_eq!((small.horizon, small.replan_interval, small.pbs_node_limit), (10, 5, 50));
        assert_eq!(config_for(&ParamValues::default(), 5_000).pbs_node_limit, 10_000);
    }

    #[test]
    fn params_reach_the_config() {
        let values = ParamValues::default()
            .with("horizon", ParamValue::Int(7))
            .with("replan_interval", ParamValue::Int(3))
            .with("nodes_per_agent", ParamValue::Int(4));
        let config = config_for(&values, 40);
        assert_eq!((config.horizon, config.replan_interval, config.pbs_node_limit), (7, 3, 160));
    }
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::task::TaskLeg;
    use crate::core::topology::ZoneMap;
    use crate::planning::heuristics::DistanceMapCache;
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
        let mut solver = RhcrPlanner::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentView {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];

        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };

        // First call should replan immediately (counter starts at W-1)
        let result = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);
        assert!(matches!(result, Decision::Plans(_)));

        // After replan, agent has a plan — simulate ECS applying it.
        for tick in 1..=4 {
            let agents_moving = vec![AgentView {
                index: 0,
                pos: IVec2::new(tick, 0),
                goal: Some(IVec2::new(4, 4)),
                has_plan: true,
                task_leg: TaskLeg::Free,
            }];
            let result =
                solver.step(&Observation { agents: &agents_moving, ..obs }, &mut cache, &mut rng);
            assert!(matches!(result, Decision::Keep), "tick {} should be Continue", tick);
        }

        // 5th call after replan should replan again
        let agents_at_4 = vec![AgentView {
            index: 0,
            pos: IVec2::new(4, 1),
            goal: Some(IVec2::new(4, 4)),
            has_plan: true,
            task_leg: TaskLeg::Free,
        }];
        let result =
            solver.step(&Observation { agents: &agents_at_4, ..obs }, &mut cache, &mut rng);
        assert!(matches!(result, Decision::Plans(_)));
    }

    #[test]
    fn pbs_partial_rate_none_before_any_step() {
        // Fresh solver has never called plan_window — rate must be None,
        // not Some(0.0), so downstream code can distinguish "no data" from
        // "zero partials over N windows".
        let cfg = RhcrConfig { horizon: 10, replan_interval: 5, pbs_node_limit: 100 };
        let solver = RhcrPlanner::new(cfg);
        assert_eq!(telemetry_value(&solver, "pbs_partial_rate"), None);
    }

    #[test]
    fn pbs_partial_rate_after_step_is_some_in_range() {
        // After at least one step() that triggered a replan, the counter
        // advances and the rate is Some(x) with x ∈ [0.0, 1.0].
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let cfg = RhcrConfig { horizon: 10, replan_interval: 1, pbs_node_limit: 500 };
        let mut solver = RhcrPlanner::new(cfg);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentView {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };

        let _ = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);

        match telemetry_value(&solver, "pbs_partial_rate") {
            Some(r) => {
                assert!(r.is_finite(), "rate must be finite");
                assert!((0.0..=1.0).contains(&r), "rate {} out of [0,1]", r);
            }
            None => panic!("expected Some(rate) after at least one replan"),
        }
    }

    #[test]
    fn pbs_partial_rate_reset_clears_counters() {
        // reset() must zero both counters so rate returns to None.
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let cfg = RhcrConfig { horizon: 10, replan_interval: 1, pbs_node_limit: 500 };
        let mut solver = RhcrPlanner::new(cfg);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![AgentView {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };

        let _ = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);
        assert!(
            telemetry_value(&solver, "pbs_partial_rate").is_some(),
            "must have rate after step"
        );

        solver.reset();
        assert_eq!(telemetry_value(&solver, "pbs_partial_rate"), None, "reset must clear counters");
    }

    #[test]
    fn rhcr_auto_config_uses_reference_defaults() {
        // RHCR README / driver.cpp:116-119: h = 5, w = 10, at any scale.
        for n in [5, 400] {
            let cfg = config_for(&ParamValues::default(), n);
            assert_eq!(cfg.replan_interval, constants::RHCR_SIMULATION_WINDOW_DEFAULT);
            assert_eq!(cfg.horizon, constants::RHCR_PLANNING_HORIZON_DEFAULT);
            assert_eq!(
                cfg.pbs_node_limit,
                (n * constants::PBS_NODES_PER_AGENT)
                    .clamp(constants::PBS_MIN_NODE_LIMIT, constants::PBS_MAX_NODE_LIMIT)
            );
        }
    }

    #[test]
    fn rhcr_unsolvable_window_counts_as_partial() {
        // Head-on swap in a 1-wide corridor: PBS can never remove the conflict.
        let grid = GridMap::new(5, 1);
        let zones = test_zones();
        let cfg = RhcrConfig { horizon: 10, replan_interval: 5, pbs_node_limit: 50 };
        let mut solver = RhcrPlanner::new(cfg);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents: Vec<AgentView> = [(0, 4), (4, 0)]
            .iter()
            .enumerate()
            .map(|(i, &(from, to))| AgentView {
                index: i,
                pos: IVec2::new(from, 0),
                goal: Some(IVec2::new(to, 0)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            })
            .collect();
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        let Decision::Plans(plans) =
            solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng)
        else {
            panic!("first step replans");
        };
        // LRA output must be collision-free.
        let p: Vec<IVec2> = plans
            .iter()
            .map(|(i, a)| a.first().map_or(agents[*i].pos, |x| x.apply(agents[*i].pos)))
            .collect();
        assert_ne!(p[0], p[1]);
        assert_eq!(telemetry_value(&solver, "pbs_partial_rate"), Some(1.0));
    }

    #[test]
    fn rhcr_congested_window_counted() {
        // Agents parked on their goals never move: every closed window is congested.
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let cfg = RhcrConfig { horizon: 10, replan_interval: 2, pbs_node_limit: 100 };
        let mut solver = RhcrPlanner::new(cfg);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents: Vec<AgentView> = (0..3)
            .map(|i| AgentView {
                index: i,
                pos: IVec2::new(i as i32, 2),
                goal: Some(IVec2::new(i as i32, 2)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            })
            .collect();
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        assert_eq!(telemetry_value(&solver, "congested_window_rate"), None);
        for _ in 0..5 {
            let _ = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);
        }
        assert_eq!(telemetry_value(&solver, "congested_window_rate"), Some(1.0));
    }

    #[test]
    fn rhcr_pbs_produces_plans() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig { horizon: 15, replan_interval: 1, pbs_node_limit: 500 };
        let mut solver = RhcrPlanner::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let agents = vec![
            AgentView {
                index: 0,
                pos: IVec2::new(0, 0),
                goal: Some(IVec2::new(4, 0)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
            AgentView {
                index: 1,
                pos: IVec2::new(0, 4),
                goal: Some(IVec2::new(4, 4)),
                has_plan: false,
                task_leg: TaskLeg::Free,
            },
        ];

        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        let result = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);
        match result {
            Decision::Plans(plans) => {
                assert_eq!(plans.len(), 2);
                assert!(!plans[0].1.is_empty());
                assert!(!plans[1].1.is_empty());
            }
            _ => panic!("expected Replan"),
        }
    }

    #[test]
    fn rhcr_between_windows_plans_only_planless_agents_around_fixed_paths() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = RhcrConfig { horizon: 10, replan_interval: 5, pbs_node_limit: 100 };
        let mut solver = RhcrPlanner::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let state = |index, pos, goal, has_plan| AgentView {
            index,
            pos,
            goal: Some(goal),
            has_plan,
            task_leg: TaskLeg::Free,
        };

        // Window at tick 0: agent 0 crosses row 2, agent 1 holds (2,0).
        let a0_goal = IVec2::new(4, 2);
        let agents = vec![
            state(0, IVec2::new(0, 2), a0_goal, false),
            state(1, IVec2::new(2, 0), IVec2::new(2, 0), false),
        ];
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        let Decision::Plans(plans) =
            solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng)
        else {
            panic!("tick 0 is a window");
        };
        let p0: Vec<Action> = plans.iter().find(|(i, _)| *i == 0).unwrap().1.to_vec();
        assert!(!p0.is_empty());

        // Tick 1: agent 0 followed its plan; agent 1 got a new goal across row 2.
        let a0_pos = p0[0].apply(IVec2::new(0, 2));
        let a1_goal = IVec2::new(2, 4);
        let agents =
            vec![state(0, a0_pos, a0_goal, true), state(1, IVec2::new(2, 0), a1_goal, false)];
        let obs = Observation { tick: 1, grid: &grid, zones: &zones, agents: &[] };
        let Decision::Plans(plans) =
            solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng)
        else {
            panic!("planless agent must be planned before the next window");
        };
        assert_eq!(plans.len(), 1, "agent 0 keeps its path");
        assert_eq!(plans[0].0, 1);
        let p1 = plans[0].1.to_vec();
        assert_eq!(telemetry_value(&solver, "extra_replans"), Some(1.0));

        // Agent 1's new path never meets agent 0's remaining path.
        let (mut x0, mut x1) = (a0_pos, IVec2::new(2, 0));
        for t in 0..10 {
            let n0 = p0.get(t + 1).map_or(x0, |a| a.apply(x0));
            let n1 = p1.get(t).map_or(x1, |a| a.apply(x1));
            assert_ne!(n0, n1, "vertex conflict at t={}", t + 1);
            assert!(!(n0 == x1 && n1 == x0), "swap at t={}", t + 1);
            (x0, x1) = (n0, n1);
        }
        assert_eq!(x1, a1_goal);
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
        let entry =
            registry.find("warehouse_single_dock").expect("warehouse_single_dock.json missing");
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

        let config = config_for(&ParamValues::default(), n);
        let mut solver = RhcrPlanner::new(config);
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

            let agent_states: Vec<AgentView> = (0..n)
                .map(|i| AgentView {
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
            let obs = Observation { tick, grid: &grid, zones: &solver_zones, agents: &[] };

            match solver.step(&Observation { agents: &agent_states, ..obs }, &mut cache, &mut rng) {
                Decision::Keep => {}
                Decision::Plans(new_plans) => {
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
        let config = config_for(&ParamValues::default(), 0);
        let mut solver = RhcrPlanner::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        let result = solver.step(&Observation { agents: &[], ..obs }, &mut cache, &mut rng);
        match result {
            Decision::Plans(plans) => assert_eq!(plans.len(), 0),
            _ => panic!("expected Replan with empty plans"),
        }
    }

    #[test]
    fn rhcr_reset_clears_state() {
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let config = config_for(&ParamValues::default(), 1);
        let mut solver = RhcrPlanner::new(config);
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);

        let agents = vec![AgentView {
            index: 0,
            pos: IVec2::ZERO,
            goal: Some(IVec2::new(4, 4)),
            has_plan: false,
            task_leg: TaskLeg::Free,
        }];
        let obs = Observation { tick: 0, grid: &grid, zones: &zones, agents: &[] };
        let _ = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);

        solver.reset();
        // After reset, first call should replan again
        let result = solver.step(&Observation { agents: &agents, ..obs }, &mut cache, &mut rng);
        assert!(matches!(result, Decision::Plans(_)));
    }

    /// Regression test for the RHCR-PBS audit (Step 2 of solver-refocus).
    ///
    /// Locks the current lazy-priority PBS throughput on a known instance so a
    /// future change to the planner (e.g. switching to eager mode) is detected.
    /// The lower bound is set generously below the measured baseline so that
    /// minor implementation drift doesn't break the test, but a regression that
    /// drops PBS to all-Wait or all-fallback behavior would trip it.
    ///
    /// Measured baseline (post RHCR-PBS fidelity port):
    /// tp = 0.435 tasks/tick on warehouse_single_dock, 40 agents, random scheduler,
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
        assert!(
            registry.find("warehouse_single_dock").is_some(),
            "warehouse_single_dock.json missing"
        );

        let config = ExperimentConfig {
            solver_name: "rhcr_pbs".into(),
            topology_name: "warehouse_single_dock".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 40,
            seed: 42,
            tick_count: 200,
            custom_map: None,
            planner_params: Default::default(),
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

    #[test]
    fn telemetry_reports_window_counters() {
        use crate::planners::toolkit::testing::{small_run, tick};
        let mut runner =
            small_run(Box::new(RhcrPlanner::new(config_for(&ParamValues::default(), 12))));
        for _ in 0..40 {
            tick(&mut runner);
        }
        let telemetry = runner.planner().telemetry();
        let value = |key: &str| telemetry.iter().find(|(k, _)| k == key).map(|&(_, v)| v);
        for key in ["pbs_partial_rate", "congested_window_rate"] {
            let rate = value(key).unwrap_or_else(|| panic!("{key} missing from {telemetry:?}"));
            assert!((0.0..=1.0).contains(&rate), "{key} = {rate}");
        }
        assert!(value("extra_replans").is_some(), "{telemetry:?}");
    }

    #[test]
    fn snapshot_round_trips_priorities() {
        // Fallback length 2, fallback priorities [1.0, 2.0], none for PBS.
        let saved = [2u32.to_le_bytes(), 1.0f32.to_le_bytes(), 2.0f32.to_le_bytes()].concat();
        let mut planner = RhcrPlanner::new(config_for(&ParamValues::default(), 12));
        assert_ne!(
            planner.snapshot().as_ref(),
            Some(&saved),
            "a fresh planner holds no priorities"
        );
        planner.restore(&saved);
        assert_eq!(planner.snapshot(), Some(saved));
    }
}
