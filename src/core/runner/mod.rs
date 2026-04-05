//! Unified simulation runner — single source of truth for the tick loop.
//!
//! `SimulationRunner` owns ALL simulation state: grid, zones, agents, solver,
//! faults, RNG, and command queue. Both the headless baseline engine and the
//! live Bevy wrapper call `tick()` to advance the simulation. Parity is
//! guaranteed by construction: there is only ONE code path.
//!
//! The runner is Bevy-free (uses `IVec2` from glam, not Bevy ECS). This makes
//! it testable without a Bevy `App` and usable in headless mode.

mod sim_agent;
pub use sim_agent::*;

mod collision;
use collision::{COLLISION_NO_AGENT, CollisionBuffers, count_paths_through_cell};

use std::collections::{HashSet, VecDeque};

use bevy::math::IVec2;
use rand::Rng;

use super::action::Action;
use super::grid::GridMap;
use super::queue::{DeliveryQueuePolicy, QueueManager};
use super::seed::SeededRng;
use super::task::{TaskAgentSnapshot, TaskLeg, TaskScheduler, recycle_goals_core};
use super::topology::ZoneMap;
use crate::constants::THROUGHPUT_WINDOW_SIZE;
use crate::fault::config::{FaultConfig, FaultSource, FaultType};
use crate::fault::scenario::{FaultSchedule, ScheduledAction};
use crate::solver::heuristics::DistanceMapCache;
use crate::solver::lifelong::{
    AgentState as SolverAgentState, LifelongSolver, SolverContext, StepResult,
};

// ---------------------------------------------------------------------------
// SimulationRunner
// ---------------------------------------------------------------------------

/// XOR salt used to derive the fault RNG seed from the main seed.
///
/// By using a dedicated RNG stream for fault detection (Weibull + intermittent),
/// the task/solver RNG stream is identical between baseline and faulted runs.
/// This makes baseline comparison valid: same seed → same task assignments,
/// same solver decisions; only fault outcomes differ.
pub const FAULT_RNG_SALT: u64 = 0x9e37_79b9_7f4a_7c15;

pub struct SimulationRunner {
    // ── Map (owned — faults can mutate grid) ──────────────────────────
    grid: GridMap,
    zones: ZoneMap,

    // ── Agents ────────────────────────────────────────────────────────
    /// All agents, indexed by agent index (deterministic order).
    pub agents: Vec<SimAgent>,

    // ── Solver ────────────────────────────────────────────────────────
    solver: Box<dyn LifelongSolver>,
    dist_cache: DistanceMapCache,

    // ── Delivery queue ───────────────────────────────────────────────
    queue_manager: QueueManager,

    // ── Faults ────────────────────────────────────────────────────────
    fault_config: FaultConfig,
    fault_schedule: FaultSchedule,
    /// Pre-sampled Weibull failure ticks per agent (inverse CDF).
    /// `weibull_failure_ticks[i]` = operational_age at which agent i fails.
    /// Sampled once at init from `fault_rng`; re-sampled on `reset()`.
    /// Empty when Weibull is disabled.
    weibull_failure_ticks: Vec<u32>,

    // ── Commands (processed at tick boundary) ─────────────────────────
    command_queue: Vec<SimCommand>,

    // ── State ─────────────────────────────────────────────────────────
    rng: SeededRng,
    /// Separate RNG stream for fault detection (Weibull + intermittent).
    ///
    /// Seeded with `rng.seed() ^ FAULT_RNG_SALT` so it is independent of the
    /// task/solver RNG stream. Both baseline and faulted runs use the same
    /// fault_rng seed, keeping task-assignment RNG identical between them.
    fault_rng: SeededRng,
    /// Current tick (incremented during each tick).
    pub tick: u64,
    /// Cumulative task completions.
    pub tasks_completed: u64,
    /// Sliding window of completion ticks for throughput calculation.
    task_completion_ticks: VecDeque<u64>,

    // ── Scratch buffers (reused across ticks to avoid per-tick allocation) ──
    solver_states_buf: Vec<SolverAgentState>,
    task_snapshots_buf: Vec<TaskAgentSnapshot>,
    /// Reusable collision resolution state (flat grid, zero alloc after first tick).
    collision: CollisionBuffers,
    /// Reusable fault events buffer.
    fault_events_buf: Vec<FaultRecord>,
    /// Number of tasks completed during the current tick (avoids deque scan).
    tick_completions: usize,
    /// Reusable buffer for scheduled fault actions to execute.
    scheduled_actions_buf: Vec<(usize, ScheduledAction)>,
    /// Reusable buffer for available (alive) agent indices.
    available_agents_buf: Vec<usize>,
    /// Reusable buffer for detected fault decisions.
    faults_buf: Vec<(usize, FaultType)>,
    /// Number of post-hoc collision violations detected (release mode only).
    /// Always 0 if solver and collision resolution are correct.
    pub collision_violations: u64,
    /// Active spatial zone strip bounds (x_min, x_max) during a zone outage.
    /// Set when ZoneLatency fires, cleared when all latency expires.
    pub zone_strip: Option<(i32, i32)>,
}

impl SimulationRunner {
    /// Create a new runner with all simulation state.
    ///
    /// Takes an already-initialized `SeededRng`. The caller is responsible for
    /// any RNG consumption before the first tick (e.g., agent placement).
    /// This ensures the RNG state entering tick 1 is identical between
    /// baseline and live paths.
    pub fn new(
        grid: GridMap,
        zones: ZoneMap,
        agents: Vec<SimAgent>,
        solver: Box<dyn LifelongSolver>,
        rng: SeededRng,
        fault_config: FaultConfig,
        fault_schedule: FaultSchedule,
    ) -> Self {
        fault_config.validate();
        let mut fault_rng = SeededRng::new(rng.seed() ^ FAULT_RNG_SALT);
        let queue_manager = QueueManager::new(&zones.queue_lines);
        let weibull_failure_ticks = if fault_config.weibull_enabled {
            Self::sample_weibull_ticks(
                agents.len(),
                fault_config.weibull_beta,
                fault_config.weibull_eta,
                &mut fault_rng,
            )
        } else {
            Vec::new()
        };
        Self {
            grid,
            zones,
            agents,
            solver,
            dist_cache: DistanceMapCache::default(),
            queue_manager,
            fault_config,
            fault_schedule,
            weibull_failure_ticks,
            command_queue: Vec::new(),
            rng,
            fault_rng,
            tick: 0,
            tasks_completed: 0,
            task_completion_ticks: VecDeque::new(),
            solver_states_buf: Vec::new(),
            task_snapshots_buf: Vec::new(),
            collision: CollisionBuffers::new(),
            fault_events_buf: Vec::new(),
            tick_completions: 0,
            scheduled_actions_buf: Vec::new(),
            available_agents_buf: Vec::new(),
            faults_buf: Vec::new(),
            collision_violations: 0,
            zone_strip: None,
        }
    }

    /// Pre-sample Weibull failure ticks using inverse CDF:
    /// `t_fail = eta * (-ln(U))^(1/beta)`, U ~ Uniform(0,1).
    ///
    /// This is mathematically exact: the resulting failure times follow the
    /// Weibull(beta, eta) distribution. Each agent gets exactly one draw.
    fn sample_weibull_ticks(
        num_agents: usize,
        beta: f32,
        eta: f32,
        rng: &mut SeededRng,
    ) -> Vec<u32> {
        let inv_beta = 1.0_f64 / beta as f64;
        let eta_f64 = eta as f64;
        (0..num_agents)
            .map(|_| {
                let u: f64 = rng.rng.random_range(f64::EPSILON..1.0_f64);
                let t = eta_f64 * (-u.ln()).powf(inv_beta);
                // Clamp to u32 range; floor to integer operational-age ticks
                (t.round() as u64).min(u32::MAX as u64) as u32
            })
            .collect()
    }

    // ── Public accessors ─────────────────────────────────────────────

    pub fn num_agents(&self) -> usize {
        self.agents.len()
    }

    pub fn grid(&self) -> &GridMap {
        &self.grid
    }

    pub fn grid_mut(&mut self) -> &mut GridMap {
        &mut self.grid
    }

    pub fn zones(&self) -> &ZoneMap {
        &self.zones
    }

    pub fn rng(&self) -> &SeededRng {
        &self.rng
    }

    pub fn rng_mut(&mut self) -> &mut SeededRng {
        &mut self.rng
    }

    pub fn fault_rng(&self) -> &SeededRng {
        &self.fault_rng
    }

    pub fn fault_rng_mut(&mut self) -> &mut SeededRng {
        &mut self.fault_rng
    }

    pub fn fault_config(&self) -> &FaultConfig {
        &self.fault_config
    }

    pub fn set_fault_enabled(&mut self, enabled: bool) {
        self.fault_config.enabled = enabled;
    }

    pub fn fault_schedule(&self) -> &FaultSchedule {
        &self.fault_schedule
    }

    pub fn weibull_failure_ticks(&self) -> &[u32] {
        &self.weibull_failure_ticks
    }

    pub fn fault_schedule_mut(&mut self) -> &mut FaultSchedule {
        &mut self.fault_schedule
    }

    pub fn solver(&self) -> &dyn LifelongSolver {
        self.solver.as_ref()
    }

    pub fn solver_mut(&mut self) -> &mut dyn LifelongSolver {
        self.solver.as_mut()
    }

    /// Instantaneous throughput at the current tick.
    /// Uses the pre-tracked count (O(1)) instead of scanning the deque.
    pub fn throughput_current(&self) -> f64 {
        self.tick_completions as f64
    }

    /// Instantaneous throughput at a specific tick. O(1) for the current tick,
    /// falls back to deque scan for historical ticks (rare — only in tests).
    pub fn throughput(&self, tick: u64) -> f64 {
        if tick == self.tick {
            self.tick_completions as f64
        } else {
            self.task_completion_ticks.iter().filter(|&&t| t == tick).count() as f64
        }
    }

    /// Read-only access to the completion_ticks window (for snapshotting).
    pub fn completion_ticks(&self) -> &VecDeque<u64> {
        &self.task_completion_ticks
    }

    /// Restore completion state from a snapshot (used after rewind).
    pub fn restore_completion_state(
        &mut self,
        tasks_completed: u64,
        completion_ticks: VecDeque<u64>,
    ) {
        self.tasks_completed = tasks_completed;
        self.task_completion_ticks = completion_ticks;
    }

    /// Enqueue a command to be processed at the start of the next tick.
    pub fn enqueue_command(&mut self, cmd: SimCommand) {
        self.command_queue.push(cmd);
    }

    /// Clear transient state (command queue, distance cache, rebuild queues).
    /// Used after rewind to avoid stale state from the pre-rewind timeline.
    /// Rebuilds QueueManager occupancy from agent task legs so agents in
    /// Queuing/TravelLoaded states retain their queue positions.
    pub fn clear_transient_state(&mut self) {
        self.command_queue.clear();
        self.dist_cache = DistanceMapCache::default();
        self.queue_manager.rebuild_from_agents(&self.agents, &self.zones.queue_lines);
    }

    /// Reset the runner for a new simulation (keeps agents, resets state).
    pub fn reset(&mut self) {
        self.tick = 0;
        self.tasks_completed = 0;
        self.tick_completions = 0;
        self.task_completion_ticks.clear();
        self.solver.reset();
        self.dist_cache = DistanceMapCache::default();
        self.queue_manager.reset(&self.zones.queue_lines);
        self.command_queue.clear();
        self.scheduled_actions_buf.clear();
        self.available_agents_buf.clear();
        self.faults_buf.clear();
        self.zone_strip = None;
        // Re-sample Weibull failure ticks with reset fault_rng
        if self.fault_config.weibull_enabled {
            self.fault_rng = SeededRng::new(self.rng.seed() ^ FAULT_RNG_SALT);
            self.weibull_failure_ticks = Self::sample_weibull_ticks(
                self.agents.len(),
                self.fault_config.weibull_beta,
                self.fault_config.weibull_eta,
                &mut self.fault_rng,
            );
        }
    }

    // ── Main tick ────────────────────────────────────────────────────

    /// Advance the simulation by one tick.
    ///
    /// This is the **single source of truth** for the tick loop. The ordering
    /// matches the ECS system chain:
    ///
    /// 1. Process queued commands (deterministic, at tick boundary)
    /// 2. Execute fault schedule (timed scenario events)
    /// 3. Apply latency faults (force Wait on latency-affected agents)
    /// 4. Collision resolution + position update
    /// 5. Tick increment
    /// 6. Task state machine (recycle_goals)
    /// 7. Solver step (pathfinding)
    /// 8. Heat accumulation + fault detection (wear-based model)
    /// 9. Replan after new faults
    pub fn tick(
        &mut self,
        scheduler: &dyn TaskScheduler,
        queue_policy: &dyn DeliveryQueuePolicy,
    ) -> TickResult {
        // Reuse fault_events buffer (clear, not deallocate)
        let mut fault_events = std::mem::take(&mut self.fault_events_buf);
        fault_events.clear();

        // Reset per-tick completion counter
        self.tick_completions = 0;

        // Phase 0: Tick increment (first, so all phases see the current tick number)
        self.tick += 1;

        // Phase 1: Process queued commands
        self.process_commands(&mut fault_events);

        // Phase 2: Execute fault schedule (scenario events)
        self.execute_fault_schedule(&mut fault_events);

        // Phase 3: Apply latency (force Wait, decrement counters)
        self.apply_latency_faults();

        // Phase 4: Collision resolution + position update (zero-alloc via flat grid)
        let moves = self.resolve_collisions_fast();

        // Phase 5: Task state machine (TravelToLoad→Loading, TravelToDeliver→Idle)
        let just_loaded = self.recycle_goals(scheduler);

        // Phase 5.5: Queue management
        self.run_queue_manager(queue_policy, &just_loaded);

        // Phase 6: Solver step
        self.run_solver();

        // Phase 7: Fault pipeline (heat + detect)
        self.run_fault_pipeline(&mut fault_events);

        // Phase 8: Replan agents whose paths cross new obstacles
        if !fault_events.is_empty() {
            self.replan_after_fault();
        }

        // Phase 10: Build result + return fault_events buffer
        let result = self.build_result(moves, &mut fault_events);
        self.fault_events_buf = fault_events;
        result
    }

    // ── Private phases ───────────────────────────────────────────────

    /// Drain the command queue and apply each command immediately.
    fn process_commands(&mut self, fault_events: &mut Vec<FaultRecord>) {
        if self.command_queue.is_empty() {
            return;
        }
        // Take ownership to avoid borrow conflict with &mut self fields
        let mut commands = std::mem::take(&mut self.command_queue);
        for cmd in commands.drain(..) {
            match cmd {
                SimCommand::KillAgent { index, source } => {
                    if index < self.agents.len() && self.agents[index].alive {
                        let pos = self.agents[index].pos;
                        let invalidated = count_paths_through_cell(&self.agents, pos, index);
                        self.agents[index].alive = false;
                        self.agents[index].planned_path.clear();
                        self.grid.set_obstacle(pos);
                        fault_events.push(FaultRecord {
                            agent_index: index,
                            fault_type: FaultType::Breakdown,
                            source,
                            tick: self.tick,
                            position: pos,
                            paths_invalidated: invalidated,
                        });
                    }
                }
                SimCommand::PlaceObstacle(cell) => {
                    if self.grid.is_in_bounds(cell) && self.grid.is_walkable(cell) {
                        self.grid.set_obstacle(cell);
                    }
                }
                SimCommand::InjectLatency { agent_index, duration, source } => {
                    if agent_index < self.agents.len() && self.agents[agent_index].alive {
                        self.agents[agent_index].latency_remaining = duration;
                        let pos = self.agents[agent_index].pos;
                        fault_events.push(FaultRecord {
                            agent_index,
                            fault_type: FaultType::Latency,
                            source,
                            tick: self.tick,
                            position: pos,
                            paths_invalidated: 0,
                        });
                    }
                }
            }
        }
        // Return the (now empty) queue buffer for reuse
        self.command_queue = commands;
    }

    /// Execute timed fault schedule events at their designated tick.
    fn execute_fault_schedule(&mut self, fault_events: &mut Vec<FaultRecord>) {
        if !self.fault_schedule.initialized {
            return;
        }

        let tick = self.tick;
        let n = self.agents.len();

        // Collect events to fire (need indices to mark fired)
        self.scheduled_actions_buf.clear();
        for (i, event) in self.fault_schedule.events.iter().enumerate() {
            if !event.fired && event.tick == tick {
                self.scheduled_actions_buf.push((i, event.action.clone()));
            }
        }

        for (event_idx, action) in self.scheduled_actions_buf.drain(..) {
            self.fault_schedule.events[event_idx].fired = true;

            match action {
                ScheduledAction::KillRandomAgents(count) => {
                    // Collect alive agent indices
                    self.available_agents_buf.clear();
                    self.available_agents_buf.extend((0..n).filter(|&i| self.agents[i].alive));
                    let kill_count = count.min(self.available_agents_buf.len());

                    for _ in 0..kill_count {
                        if self.available_agents_buf.is_empty() {
                            break;
                        }
                        let idx = self.rng.rng.random_range(0..self.available_agents_buf.len());
                        let agent_idx = self.available_agents_buf.swap_remove(idx);
                        let pos = self.agents[agent_idx].pos;
                        let invalidated = count_paths_through_cell(&self.agents, pos, agent_idx);
                        self.agents[agent_idx].alive = false;
                        self.agents[agent_idx].planned_path.clear();
                        self.grid.set_obstacle(pos);
                        fault_events.push(FaultRecord {
                            agent_index: agent_idx,
                            fault_type: FaultType::Breakdown,
                            source: FaultSource::Scheduled,
                            tick,
                            position: pos,
                            paths_invalidated: invalidated,
                        });
                    }
                }
                ScheduledAction::ZoneLatency { duration } => {
                    // Spatial zone outage: divide the map into 4 equal vertical strips and select
                    // one strip using fault_rng (solver-independent, deterministic per seed).
                    // Models: network dead zone or power failure in a specific warehouse aisle section.
                    let grid_width = self.grid.width;
                    let strip_width = (grid_width / 4).max(1);
                    let strip_idx = self.fault_rng.rng.random_range(0i32..4);
                    let x_min = strip_idx * strip_width;
                    let x_max = if strip_idx == 3 { grid_width } else { x_min + strip_width };

                    self.zone_strip = Some((x_min, x_max));

                    for i in 0..n {
                        if !self.agents[i].alive {
                            continue;
                        }
                        let x = self.agents[i].pos.x;
                        if x >= x_min && x < x_max {
                            self.agents[i].latency_remaining = duration;
                            fault_events.push(FaultRecord {
                                agent_index: i,
                                fault_type: FaultType::Latency,
                                source: FaultSource::Scheduled,
                                tick,
                                position: self.agents[i].pos,
                                paths_invalidated: 0,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Apply latency faults: force Wait by clearing planned path, decrement counter.
    /// Clears zone_strip when all latency expires.
    fn apply_latency_faults(&mut self) {
        for agent in &mut self.agents {
            if agent.latency_remaining > 0 && agent.alive {
                agent.planned_path.clear();
                agent.latency_remaining = agent.latency_remaining.saturating_sub(1);
            }
        }
        // Clear zone strip overlay when no latency remains
        if self.zone_strip.is_some()
            && !self.agents.iter().any(|a| a.alive && a.latency_remaining > 0)
        {
            self.zone_strip = None;
        }
    }

    /// Optimized collision resolution using flat grid-indexed arrays.
    /// Zero allocation after first tick (all buffers reused).
    fn resolve_collisions_fast(&mut self) -> Vec<AgentTickResult> {
        let n = self.agents.len();
        let grid_w = self.grid.width;
        let grid_h = self.grid.height;

        // Take collision buffers to avoid borrow conflict with self
        let mut col = std::mem::take(&mut self.collision);
        col.ensure_size(grid_w, grid_h);

        // Build moves buffer (reuse)
        col.moves.clear();
        for a in &self.agents {
            if !a.alive {
                col.moves.push((a.pos, Action::Wait, a.pos, false));
            } else {
                let action = a.planned_path.front().copied().unwrap_or(Action::Wait);
                let new_pos = action.apply(a.pos);
                let (target, effective_action) = if self.grid.is_walkable(new_pos) {
                    (new_pos, action)
                } else {
                    (a.pos, Action::Wait)
                };
                col.moves.push((a.pos, effective_action, target, false));
            }
        }

        // Dead agent positions — mark in flat grid for O(1) lookup
        col.clear_dead();
        let mut has_dead = false;
        for a in &self.agents {
            if !a.alive {
                let idx = col.idx(a.pos);
                if idx < col.grid_size && !col.dead_cell[idx] {
                    col.dead_cell[idx] = true;
                    col.dirty_dead.push(idx);
                }
                has_dead = true;
            }
        }

        // Iterative collision resolution (converges in ≤n iterations)
        let mut changed = true;
        let max_iters = n + 1; // safety bound
        let mut iter = 0;
        while changed && iter < max_iters {
            changed = false;
            iter += 1;

            // --- Vertex conflicts: two agents targeting same cell ---
            // Build target map (flat grid, lazy clear)
            col.clear_targets();
            for (i, m) in col.moves.iter().enumerate() {
                let idx = col.idx(m.2);
                if idx < col.grid_size {
                    col.target_count[idx] += 1;
                    if col.target_count[idx] == 1 {
                        col.dirty_targets.push(idx);
                    }
                    // Winner = agent that's staying in place, fallback = first agent
                    if m.2 == m.0 || col.target_agent[idx] == COLLISION_NO_AGENT {
                        col.target_agent[idx] = i as u32;
                    }
                }
            }

            for i in 0..n {
                let target = col.moves[i].2;
                let idx = col.idx(target);
                if idx < col.grid_size && col.target_count[idx] > 1 {
                    let winner = col.target_agent[idx] as usize;
                    if i != winner && col.moves[i].2 != col.moves[i].0 {
                        col.moves[i].1 = Action::Wait;
                        col.moves[i].2 = col.moves[i].0;
                        col.moves[i].3 = true;
                        changed = true;
                    }
                }
            }

            // --- Edge swaps: A→B and B→A ---
            col.clear_sources();
            for (i, m) in col.moves.iter().enumerate() {
                if m.2 != m.0 {
                    let idx = col.idx(m.0);
                    if idx < col.grid_size {
                        col.source_agent[idx] = i as u32;
                        col.dirty_sources.push(idx);
                    }
                }
            }
            for i in 0..n {
                if col.moves[i].2 == col.moves[i].0 {
                    continue;
                }
                let target_idx = col.idx(col.moves[i].2);
                if target_idx < col.grid_size {
                    let j = col.source_agent[target_idx] as usize;
                    if j < n && j > i {
                        let j_target_idx = col.idx(col.moves[j].2);
                        let i_source_idx = col.idx(col.moves[i].0);
                        if j_target_idx == i_source_idx {
                            // Edge swap: force higher-index agent to wait
                            col.moves[j].1 = Action::Wait;
                            col.moves[j].2 = col.moves[j].0;
                            col.moves[j].3 = true;
                            changed = true;
                        }
                    }
                }
            }

            // --- Dead agent collisions (O(1) flat grid lookup) ---
            if has_dead {
                let grid_w = col.grid_w;
                let grid_size = col.grid_size;
                for m in col.moves.iter_mut() {
                    if m.2 != m.0 {
                        let dead_idx = (m.2.y * grid_w + m.2.x) as usize;
                        if dead_idx < grid_size && col.dead_cell[dead_idx] {
                            m.1 = Action::Wait;
                            m.2 = m.0;
                            m.3 = true;
                            changed = true;
                        }
                    }
                }
            }
        }

        // Put buffers back
        self.collision = col;

        // Apply resolved moves
        let mut moves = Vec::with_capacity(n);
        for i in 0..n {
            let (_, action, target, was_forced) = self.collision.moves[i];
            let agent = &mut self.agents[i];
            if agent.alive {
                agent.planned_path.pop_front();

                if target != agent.pos && !self.grid.is_walkable(target) {
                    agent.planned_path.clear();
                } else {
                    agent.pos = target;
                }

                agent.last_was_forced = was_forced;
                if was_forced {
                    agent.planned_path.clear();
                }

                agent.last_action = action;
            }

            moves.push(AgentTickResult { new_pos: agent.pos, action, was_forced });
        }

        // ── Post-hoc collision validator (LoRR-inspired) ────────────────
        // Independent safety net: verify no two alive agents share a position
        // AFTER collision resolution applied moves. This catches bugs in the
        // resolution logic itself, not just in the solver.
        #[cfg(debug_assertions)]
        {
            let mut seen_positions: std::collections::HashMap<IVec2, usize> =
                std::collections::HashMap::with_capacity(n);
            for (i, a) in self.agents.iter().enumerate() {
                if !a.alive {
                    continue;
                }
                if let Some(&prev) = seen_positions.get(&a.pos) {
                    panic!(
                        "POST-HOC COLLISION VALIDATOR: tick {} — agents {} and {} \
                         both at {:?} after collision resolution. \
                         This is a bug in resolve_collisions_fast().",
                        self.tick, prev, i, a.pos
                    );
                }
                seen_positions.insert(a.pos, i);
            }
        }

        // Release-mode: lightweight check with counter (no HashMap)
        #[cfg(not(debug_assertions))]
        {
            // Use the collision grid we already have for O(1) duplicate detection
            let col = &mut self.collision;
            col.clear_targets();
            let mut collision_found = false;
            for (i, a) in self.agents.iter().enumerate() {
                if !a.alive {
                    continue;
                }
                let idx = col.idx(a.pos);
                if idx < col.grid_size {
                    if col.target_count[idx] > 0 {
                        collision_found = true;
                        // Log but don't panic in release — the simulation can
                        // continue, but the collision is recorded for analysis.
                        #[cfg(not(target_arch = "wasm32"))]
                        eprintln!(
                            "WARNING: post-hoc collision at tick {} pos {:?} (agent {})",
                            self.tick, a.pos, i
                        );
                    }
                    col.target_count[idx] += 1;
                    col.dirty_targets.push(idx);
                }
            }
            if collision_found {
                self.collision_violations += 1;
            }
        }

        moves
    }

    /// Task state machine: recycle goals, assign new tasks.
    /// Returns indices of agents that just entered Loading (must be skipped
    /// by the queue manager this tick to enforce 1-tick Loading dwell).
    fn recycle_goals(&mut self, scheduler: &dyn TaskScheduler) -> Vec<usize> {
        // Temporarily take scratch buffer to avoid borrow conflicts
        let mut task_input = std::mem::take(&mut self.task_snapshots_buf);
        task_input.clear();
        task_input.extend(self.agents.iter().map(|a| TaskAgentSnapshot {
            pos: a.pos,
            goal: a.goal,
            task_leg: a.task_leg.clone(),
            alive: a.alive,
            frozen: a.latency_remaining > 0,
        }));

        let recycle =
            recycle_goals_core(&task_input, scheduler, &self.zones, &mut self.rng.rng, self.tick);

        // Put scratch buffer back
        self.task_snapshots_buf = task_input;

        // Apply task updates
        for (i, update) in recycle.updates.iter().enumerate() {
            self.agents[i].task_leg = update.task_leg.clone();
            self.agents[i].goal = update.goal;
            if update.path_cleared {
                self.agents[i].planned_path.clear();
            }
        }

        for &completion_tick in &recycle.completion_ticks {
            self.record_completion(completion_tick);
        }

        recycle.just_loaded
    }

    /// Queue management: compact, arrivals, promote, new joins, reroute.
    ///
    /// `just_loaded` contains agent indices that just entered Loading this tick.
    /// These agents are skipped by the queue manager to enforce a 1-tick Loading
    /// dwell (makes the Loading state visible in the UI).
    ///
    /// When the topology has no queue lines, Loading agents get direct delivery
    /// assignment (fallback to pre-queue behavior without the occupied-cell bug).
    fn run_queue_manager(&mut self, queue_policy: &dyn DeliveryQueuePolicy, just_loaded: &[usize]) {
        if self.zones.queue_lines.is_empty() {
            // No queue lines — assign delivery directly to Loading agents.
            // This is the fallback for topologies without directed queues.
            self.assign_delivery_direct(just_loaded);
            return;
        }

        // Reroute agents in queues with blocked delivery cells
        let mut reroute_changed = Vec::new();
        self.queue_manager.reroute_blocked_agents(
            &mut self.agents,
            &self.zones.queue_lines,
            queue_policy,
            &mut reroute_changed,
        );

        // Main queue tick: compact, arrivals, promote, new joins
        let _changed = self.queue_manager.tick(
            &mut self.agents,
            &self.zones.queue_lines,
            queue_policy,
            just_loaded,
            &mut self.rng.rng,
        );
    }

    /// Fallback delivery assignment for topologies without queue lines.
    ///
    /// Loading agents at their pickup cell get assigned a free delivery cell directly
    /// (transition to TravelToDeliver). If no delivery cell is free, they stay in Loading.
    /// Agents in `just_loaded` are skipped (must dwell in Loading for 1 tick first).
    fn assign_delivery_direct(&mut self, just_loaded: &[usize]) {
        // Collect delivery goals already claimed by en-route agents
        let mut used_goals: HashSet<IVec2> =
            self.agents.iter().filter(|a| a.alive && a.pos != a.goal).map(|a| a.goal).collect();

        // Also include delivery cells occupied by agents doing delivery
        for a in &self.agents {
            if a.alive
                && matches!(a.task_leg, TaskLeg::TravelLoaded { .. })
                && let TaskLeg::TravelLoaded { to, .. } = &a.task_leg
            {
                used_goals.insert(*to);
            }
        }

        for i in 0..self.agents.len() {
            let agent = &self.agents[i];
            if !agent.alive || agent.pos != agent.goal {
                continue;
            }
            if !matches!(agent.task_leg, TaskLeg::Loading(_)) {
                continue;
            }
            // Skip agents that just entered Loading this tick
            if just_loaded.contains(&i) {
                continue;
            }

            // Find a free delivery cell
            let pickup = match &agent.task_leg {
                TaskLeg::Loading(p) => *p,
                _ => continue,
            };

            let delivery = self
                .zones
                .delivery_cells
                .iter()
                .copied()
                .filter(|c| !used_goals.contains(c) && *c != agent.pos)
                .min_by_key(|c| (c.x - agent.pos.x).abs() + (c.y - agent.pos.y).abs());

            if let Some(delivery_cell) = delivery {
                used_goals.insert(delivery_cell);
                self.agents[i].task_leg = TaskLeg::TravelLoaded { from: pickup, to: delivery_cell };
                self.agents[i].goal = delivery_cell;
                self.agents[i].planned_path.clear();
            }
            // else: stay in Loading, retry next tick
        }
    }

    /// Solver step: plan paths for agents that need them.
    /// Dead agents are excluded — they are grid obstacles, not participants.
    fn run_solver(&mut self) {
        let n = self.agents.len();

        // Temporarily take scratch buffer to avoid borrow conflicts
        let mut agent_states = std::mem::take(&mut self.solver_states_buf);
        agent_states.clear();
        agent_states.extend(self.agents.iter().enumerate().filter(|(_, a)| a.alive).map(
            |(i, a)| SolverAgentState {
                index: i,
                pos: a.pos,
                goal: Some(a.goal),
                has_plan: a.has_plan(),
                task_leg: a.task_leg.clone(),
            },
        ));

        let ctx =
            SolverContext { grid: &self.grid, zones: &self.zones, tick: self.tick, num_agents: n };

        match self.solver.step(&ctx, &agent_states, &mut self.dist_cache, &mut self.rng) {
            StepResult::Replan(plans) => {
                for (idx, actions) in plans {
                    if *idx < n {
                        self.agents[*idx].planned_path.clear();
                        self.agents[*idx].planned_path.extend(actions.iter().copied());
                    }
                }
            }
            StepResult::Continue => {}
        }

        // Apply goal overrides from solvers that swap tasks (e.g. TPTS).
        let overrides = self.solver.drain_goal_overrides();
        for (idx, new_goal) in overrides {
            if idx < n && self.agents[idx].alive {
                self.agents[idx].goal = new_goal;
            }
        }

        // Put scratch buffer back
        self.solver_states_buf = agent_states;

        // Evict stale cache entries periodically
        if self.tick.is_multiple_of(100) {
            let goals: Vec<IVec2> = self.agents.iter().map(|a| a.goal).collect();
            self.dist_cache.retain_goals(&goals);
        }
    }

    fn run_fault_pipeline(&mut self, fault_events: &mut Vec<FaultRecord>) {
        if !self.fault_config.enabled {
            return;
        }
        if self.fault_config.weibull_enabled {
            self.update_agent_wear();
            self.detect_weibull_faults(fault_events);
        }
        if self.fault_config.intermittent_enabled {
            self.check_intermittent_faults(fault_events);
        }
    }

    /// Update each alive agent's operational age and visual stress indicator.
    ///
    /// `operational_age` increments only on actual moves (distance-based wear).
    /// Grid-blocked moves (e.g., obstacle from a dead agent) are converted to
    /// `Action::Wait` during collision resolution, so they do not count as wear.
    /// `heat` shows individual progress toward this agent's sampled failure tick:
    /// 0.0 = fresh, 1.0 = about to fail. This replaces the old Weibull CDF which
    /// showed population-level failure probability, causing agents to visually die
    /// at random heat levels.
    fn update_agent_wear(&mut self) {
        for i in 0..self.agents.len() {
            if !self.agents[i].alive {
                continue;
            }
            if matches!(self.agents[i].last_action, Action::Move(_)) {
                self.agents[i].operational_age = self.agents[i].operational_age.saturating_add(1);
            }
            // Visual stress: progress toward individual failure tick (0 → 1 at death).
            if i < self.weibull_failure_ticks.len() && self.weibull_failure_ticks[i] > 0 {
                self.agents[i].heat = (self.agents[i].operational_age as f32
                    / self.weibull_failure_ticks[i] as f32)
                    .min(1.0);
            }
        }
    }

    /// Detect permanent Weibull wear failures using pre-sampled failure ticks.
    ///
    /// At init, each agent's failure tick was sampled via inverse CDF:
    /// `t_fail = eta * (-ln(U))^(1/beta)`, U ~ Uniform(0,1).
    /// Here we simply compare `operational_age >= failure_tick`.
    ///
    /// No per-tick RNG consumption — failure times are deterministic from init.
    fn detect_weibull_faults(&mut self, fault_events: &mut Vec<FaultRecord>) {
        let n = self.agents.len();
        let tick = self.tick;

        self.faults_buf.clear();
        for i in 0..n {
            if !self.agents[i].alive {
                continue;
            }
            if i < self.weibull_failure_ticks.len()
                && self.agents[i].operational_age >= self.weibull_failure_ticks[i]
            {
                self.faults_buf.push((i, FaultType::Overheat));
            }
        }

        // Process faults by index to avoid borrow conflict with count_paths_through_cell.
        let fault_count = self.faults_buf.len();
        for fi in 0..fault_count {
            let (i, ft) = self.faults_buf[fi];
            let pos = self.agents[i].pos;
            // Count paths crossing dead cell BEFORE clearing (captures obstacle cascade impact)
            let invalidated = count_paths_through_cell(&self.agents, pos, i);
            self.agents[i].alive = false;
            self.agents[i].planned_path.clear();
            self.grid.set_obstacle(pos);
            fault_events.push(FaultRecord {
                agent_index: i,
                fault_type: ft,
                source: FaultSource::Automatic,
                tick,
                position: pos,
                paths_invalidated: invalidated,
            });
        }
        self.faults_buf.clear();
    }

    /// Inject temporary intermittent faults via exponential inter-arrival times.
    ///
    /// Each alive agent independently samples its next fault tick from Exp(1/mtbf).
    /// When the fault fires, `latency_remaining` is set (agent unavailable for N ticks).
    /// The agent is NOT killed -- it recovers automatically when latency expires.
    fn check_intermittent_faults(&mut self, fault_events: &mut Vec<FaultRecord>) {
        let n = self.agents.len();
        let tick = self.tick;
        let mtbf = self.fault_config.intermittent_mtbf_ticks as f64;
        let recovery = self.fault_config.intermittent_recovery_ticks;

        // Phase 1: initialize next_fault_tick for agents that don't have one.
        for i in 0..n {
            if !self.agents[i].alive || self.agents[i].next_fault_tick.is_some() {
                continue;
            }
            // Exponential inter-arrival: delay = -mtbf * ln(U), U ~ Uniform(0,1).
            // Capped at 10× mtbf to prevent extreme outliers from producing
            // 0-fault runs that silently drop from statistical summaries.
            // Uses fault_rng so task/solver RNG stream is unaffected by intermittent init.
            let u = self.fault_rng.rng.random_range(f64::EPSILON..1.0_f64);
            let max_delay = (mtbf * 10.0).round() as u64;
            let delay = ((-(mtbf) * u.ln()).round() as u64).min(max_delay).max(1);
            self.agents[i].next_fault_tick = Some(tick + delay);
        }

        // Phase 2: fire faults and resample next interval.
        for i in 0..n {
            if !self.agents[i].alive {
                continue;
            }
            let Some(next) = self.agents[i].next_fault_tick else {
                continue;
            };
            if tick < next {
                continue;
            }
            // Inject latency -- temporary unavailability, not death.
            self.agents[i].latency_remaining = recovery;
            // Resample next fault (fault_rng stream). Capped at 10× mtbf.
            let u = self.fault_rng.rng.random_range(f64::EPSILON..1.0_f64);
            let max_delay = (mtbf * 10.0).round() as u64;
            let delay = ((-(mtbf) * u.ln()).round() as u64).min(max_delay).max(1);
            self.agents[i].next_fault_tick = Some(tick + delay);

            let pos = self.agents[i].pos;
            fault_events.push(FaultRecord {
                agent_index: i,
                fault_type: FaultType::Latency,
                source: FaultSource::Automatic,
                tick,
                position: pos,
                paths_invalidated: 0,
            });
        }
    }

    /// Clear planned paths for agents whose routes cross new obstacles.
    fn replan_after_fault(&mut self) {
        for agent in &mut self.agents {
            if !agent.alive || agent.planned_path.is_empty() {
                continue;
            }

            let mut pos = agent.pos;
            let mut blocked = false;
            for action in &agent.planned_path {
                pos = action.apply(pos);
                if !self.grid.is_walkable(pos) {
                    blocked = true;
                    break;
                }
            }

            if blocked {
                agent.planned_path.clear();
            }
        }
    }

    /// Record a task completion (updates internal counters + per-tick count).
    fn record_completion(&mut self, tick: u64) {
        self.tasks_completed += 1;
        self.tick_completions += 1;
        self.task_completion_ticks.push_back(tick);
        while self.task_completion_ticks.len() > THROUGHPUT_WINDOW_SIZE {
            self.task_completion_ticks.pop_front();
        }
    }

    /// Build the TickResult from current state.
    /// Takes `fault_events` by `&mut Vec` so we can move the contents out
    /// (zero-copy) instead of cloning. The caller's Vec is left empty.
    fn build_result(
        &self,
        moves: Vec<AgentTickResult>,
        fault_events: &mut Vec<FaultRecord>,
    ) -> TickResult {
        // Single pass over agents for all aggregate stats
        let mut alive_count = 0usize;
        let mut idle_count = 0usize;
        let mut all_at_goal = true;
        let mut heat_sum = 0.0f32;
        for a in &self.agents {
            if a.alive {
                alive_count += 1;
                if matches!(a.task_leg, TaskLeg::Free) {
                    idle_count += 1;
                }
                if !a.has_reached_goal() {
                    all_at_goal = false;
                }
                heat_sum += a.heat;
            }
        }
        let dead_count = self.agents.len() - alive_count;
        let heat_avg = if alive_count > 0 { heat_sum / alive_count as f32 } else { 0.0 };

        // Use pre-tracked count (O(1) instead of scanning deque)
        let throughput = self.tick_completions as f64;

        // Build completion ticks from the count (all at self.tick)
        let completion_ticks: Vec<u64> = if self.tick_completions > 0 {
            vec![self.tick; self.tick_completions]
        } else {
            Vec::new()
        };

        // Move fault events out (zero-copy) — caller's Vec left empty
        let events = std::mem::take(fault_events);

        TickResult {
            moves,
            completion_ticks,
            tasks_completed: self.tasks_completed,
            throughput,
            tick: self.tick,
            idle_count,
            all_at_goal,
            fault_events: events,
            alive_count,
            dead_count,
            heat_avg,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::queue::ClosestQueuePolicy;
    use crate::core::task::{RandomScheduler, TaskLeg};
    use crate::core::topology::{ZoneMap, ZoneType};
    use std::collections::HashMap;

    // ── Test fixtures ────────────────────────────────────────────────

    fn test_grid() -> GridMap {
        GridMap::new(8, 8)
    }

    fn test_zones() -> ZoneMap {
        let mut zone_type = HashMap::new();
        let mut pickup_cells = Vec::new();
        let mut delivery_cells = Vec::new();
        let mut corridor_cells = Vec::new();

        for x in 0..8 {
            for y in 0..8 {
                let pos = IVec2::new(x, y);
                if y < 2 {
                    zone_type.insert(pos, ZoneType::Delivery);
                    delivery_cells.push(pos);
                } else if y == 3 || y == 5 {
                    zone_type.insert(pos, ZoneType::Pickup);
                    pickup_cells.push(pos);
                } else {
                    zone_type.insert(pos, ZoneType::Corridor);
                    corridor_cells.push(pos);
                }
            }
        }

        ZoneMap {
            pickup_cells,
            delivery_cells,
            corridor_cells,
            recharging_cells: Vec::new(),
            zone_type,
            queue_lines: Vec::new(),
        }
    }

    fn test_solver() -> Box<dyn LifelongSolver> {
        Box::new(crate::solver::pibt::PibtLifelongSolver::new())
    }

    fn disabled_fault_config() -> FaultConfig {
        FaultConfig { enabled: false, ..Default::default() }
    }

    fn default_schedule() -> FaultSchedule {
        FaultSchedule::default()
    }

    fn test_rng() -> SeededRng {
        SeededRng::new(42)
    }

    fn make_runner(agents: Vec<SimAgent>) -> SimulationRunner {
        SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            disabled_fault_config(),
            default_schedule(),
        )
    }

    // ── Lifecycle ────────────────────────────────────────────────────

    #[test]
    fn initializes_with_zero_tick_and_tasks() {
        let runner =
            make_runner(vec![SimAgent::new(IVec2::new(1, 1)), SimAgent::new(IVec2::new(3, 3))]);
        assert_eq!(runner.num_agents(), 2);
        assert_eq!(runner.tick, 0);
        assert_eq!(runner.tasks_completed, 0);
    }

    #[test]
    fn tick_increments_monotonically() {
        let mut runner = make_runner(vec![SimAgent::new(IVec2::new(1, 1))]);
        let s = RandomScheduler;
        assert_eq!(runner.tick(&s, &ClosestQueuePolicy).tick, 1);
        assert_eq!(runner.tick(&s, &ClosestQueuePolicy).tick, 2);
    }

    #[test]
    fn agents_receive_tasks_after_first_tick() {
        let mut runner =
            make_runner(vec![SimAgent::new(IVec2::new(1, 1)), SimAgent::new(IVec2::new(3, 3))]);
        runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        let non_idle =
            runner.agents.iter().filter(|a| !matches!(a.task_leg, TaskLeg::Free)).count();
        assert!(non_idle > 0);
    }

    #[test]
    fn reset_clears_all_state() {
        let mut runner = make_runner(vec![SimAgent::new(IVec2::new(1, 1))]);
        for _ in 0..5 {
            runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        }
        runner.reset();
        assert_eq!(runner.tick, 0);
        assert_eq!(runner.tasks_completed, 0);
        assert_eq!(runner.task_completion_ticks.len(), 0);
    }

    // ── Throughput tracking ──────────────────────────────────────────

    #[test]
    fn throughput_tracks_per_tick_completions() {
        let mut runner = make_runner(vec![SimAgent::new(IVec2::new(1, 1))]);
        assert_eq!(runner.throughput(0), 0.0);
        runner.record_completion(5);
        assert_eq!(runner.throughput(5), 1.0);
        assert_eq!(runner.throughput(6), 0.0);
        runner.record_completion(5);
        assert_eq!(runner.throughput(5), 2.0);
    }

    #[test]
    fn completes_tasks_over_100_ticks() {
        let mut runner = make_runner(vec![
            SimAgent::new(IVec2::new(1, 1)),
            SimAgent::new(IVec2::new(3, 3)),
            SimAgent::new(IVec2::new(5, 5)),
        ]);
        for _ in 0..100 {
            runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        }
        assert!(runner.tasks_completed > 0);
    }

    // ── Collision resolution ─────────────────────────────────────────

    #[test]
    fn dead_agents_block_live_agent_movement() {
        let mut agents = vec![SimAgent::new(IVec2::new(2, 1)), SimAgent::new(IVec2::new(1, 1))];
        agents[0].alive = false;
        agents[1].goal = IVec2::new(2, 1);

        let mut grid = test_grid();
        grid.set_obstacle(IVec2::new(2, 1));

        let mut runner = SimulationRunner::new(
            grid,
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            disabled_fault_config(),
            default_schedule(),
        );
        for _ in 0..10 {
            runner.tick(&RandomScheduler, &ClosestQueuePolicy);
            assert_ne!(runner.agents[1].pos, IVec2::new(2, 1));
        }
    }

    // ── Commands ─────────────────────────────────────────────────────

    #[test]
    fn kill_command_marks_dead_and_places_obstacle() {
        let mut runner =
            make_runner(vec![SimAgent::new(IVec2::new(1, 1)), SimAgent::new(IVec2::new(3, 3))]);
        runner.enqueue_command(SimCommand::KillAgent { index: 0, source: FaultSource::Manual });
        let result = runner.tick(&RandomScheduler, &ClosestQueuePolicy);

        assert!(!runner.agents[0].alive);
        assert!(runner.grid().is_obstacle(IVec2::new(1, 1)));
        assert_eq!(result.fault_events.len(), 1);
        assert_eq!(result.dead_count, 1);
    }

    #[test]
    fn obstacle_command_blocks_cell() {
        let mut runner = make_runner(vec![SimAgent::new(IVec2::new(1, 1))]);
        runner.enqueue_command(SimCommand::PlaceObstacle(IVec2::new(4, 4)));
        runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert!(runner.grid().is_obstacle(IVec2::new(4, 4)));
    }

    #[test]
    fn latency_command_forces_wait_and_decrements() {
        let mut agents = vec![SimAgent::new(IVec2::new(1, 1))];
        agents[0].goal = IVec2::new(5, 5);
        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            disabled_fault_config(),
            default_schedule(),
        );
        runner.enqueue_command(SimCommand::InjectLatency {
            agent_index: 0,
            duration: 5,
            source: FaultSource::Manual,
        });

        let result = runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert_eq!(result.fault_events.len(), 1);
        assert_eq!(runner.agents[0].latency_remaining, 4);

        runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert_eq!(runner.agents[0].latency_remaining, 3);
    }

    // ── Fault schedule ───────────────────────────────────────────────

    #[test]
    fn burst_schedule_kills_exact_count_at_target_tick() {
        let agents: Vec<SimAgent> =
            (0..10).map(|i| SimAgent::new(IVec2::new(i % 8, i / 8))).collect();
        let schedule = FaultSchedule {
            events: vec![crate::fault::scenario::ScheduledEvent {
                tick: 5,
                action: ScheduledAction::KillRandomAgents(3),
                fired: false,
            }],
            initialized: true,
        };
        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            disabled_fault_config(),
            schedule,
        );
        for _ in 0..4 {
            let r = runner.tick(&RandomScheduler, &ClosestQueuePolicy);
            assert_eq!(r.fault_events.len(), 0);
        }
        runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert_eq!(runner.agents.iter().filter(|a| !a.alive).count(), 3);
    }

    // ── Weibull wear model ───────────────────────────────────────────

    #[test]
    fn wear_accumulates_heat_indicator() {
        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            vec![SimAgent::new(IVec2::new(1, 1))],
            test_solver(),
            test_rng(),
            FaultConfig {
                enabled: true,
                weibull_enabled: true,
                weibull_beta: 2.5,
                weibull_eta: 10000.0,
                intermittent_enabled: false,
                ..Default::default()
            },
            default_schedule(),
        );
        runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert!(runner.agents[0].heat >= 0.0);
    }

    #[test]
    fn weibull_kills_agent_at_presampled_tick() {
        // With eta=10.0 and beta=3.5, most failure ticks will be small.
        // Set operational_age above the pre-sampled failure tick to trigger death.
        let agents = vec![SimAgent::new(IVec2::new(1, 1))];

        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            FaultConfig {
                enabled: true,
                weibull_enabled: true,
                weibull_beta: 3.5,
                weibull_eta: 10.0,
                intermittent_enabled: false,
                ..Default::default()
            },
            default_schedule(),
        );
        // Set operational_age well above any plausible Weibull(3.5, 10) failure tick
        runner.agents[0].operational_age = 100_000;
        let result = runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        assert!(!runner.agents[0].alive);
        assert_eq!(result.fault_events[0].fault_type, FaultType::Overheat);
    }

    // ── Determinism ──────────────────────────────────────────────────

    #[test]
    fn same_seed_produces_identical_simulation() {
        let make = || {
            make_runner(vec![
                SimAgent::new(IVec2::new(1, 1)),
                SimAgent::new(IVec2::new(3, 3)),
                SimAgent::new(IVec2::new(5, 5)),
            ])
        };

        let mut r1 = make();
        let mut r2 = make();
        for _ in 0..50 {
            let t1 = r1.tick(&RandomScheduler, &ClosestQueuePolicy);
            let t2 = r2.tick(&RandomScheduler, &ClosestQueuePolicy);
            assert_eq!(t1.tick, t2.tick);
            assert_eq!(t1.tasks_completed, t2.tasks_completed);
            for (a, b) in r1.agents.iter().zip(r2.agents.iter()) {
                assert_eq!(a.pos, b.pos, "tick {}", t1.tick);
            }
        }
    }

    #[test]
    fn determinism_holds_with_faults() {
        let make = || {
            let agents: Vec<SimAgent> =
                (0..10).map(|i| SimAgent::new(IVec2::new(i % 8, i / 8))).collect();
            let schedule = FaultSchedule {
                events: vec![crate::fault::scenario::ScheduledEvent {
                    tick: 10,
                    action: ScheduledAction::KillRandomAgents(3),
                    fired: false,
                }],
                initialized: true,
            };
            SimulationRunner::new(
                test_grid(),
                test_zones(),
                agents,
                test_solver(),
                test_rng(),
                FaultConfig {
                    enabled: true,
                    weibull_enabled: true,
                    weibull_beta: 2.5,
                    weibull_eta: 500.0,
                    intermittent_enabled: false,
                    ..Default::default()
                },
                schedule,
            )
        };

        let mut r1 = make();
        let mut r2 = make();
        for _ in 0..50 {
            let t1 = r1.tick(&RandomScheduler, &ClosestQueuePolicy);
            let t2 = r2.tick(&RandomScheduler, &ClosestQueuePolicy);
            assert_eq!(t1.tick, t2.tick);
            assert_eq!(t1.tasks_completed, t2.tasks_completed);
            assert_eq!(t1.alive_count, t2.alive_count);
            assert_eq!(t1.fault_events.len(), t2.fault_events.len());
            for (a, b) in r1.agents.iter().zip(r2.agents.iter()) {
                assert_eq!(a.pos, b.pos, "tick {}", t1.tick);
                assert_eq!(a.alive, b.alive, "tick {}", t1.tick);
            }
        }
    }

    // ── Weibull MTTF verification ───────────────────────────────────

    /// Verify that pre-sampled Weibull failure ticks match the theoretical MTTF.
    /// MTTF = eta * Gamma(1 + 1/beta). With 10,000 agents the sample mean
    /// should be within 5% of the theoretical value.
    #[test]
    fn weibull_mttf_matches_theory() {
        use crate::core::seed::SeededRng;

        let beta = 2.5_f32;
        let eta = 500.0_f32;
        let n = 10_000;

        let mut rng = SeededRng::new(12345);
        let ticks = SimulationRunner::sample_weibull_ticks(n, beta, eta, &mut rng);

        let sample_mean: f64 = ticks.iter().map(|&t| t as f64).sum::<f64>() / n as f64;

        // Theoretical MTTF = eta * Gamma(1 + 1/beta)
        // For beta=2.5: Gamma(1.4) ≈ 0.88726
        // MTTF ≈ 500 * 0.88726 ≈ 443.63
        let gamma_1_plus_inv_beta = gamma_approx(1.0 + 1.0 / beta as f64);
        let theoretical_mttf = eta as f64 * gamma_1_plus_inv_beta;

        let relative_error = ((sample_mean - theoretical_mttf) / theoretical_mttf).abs();
        assert!(
            relative_error < 0.05,
            "MTTF mismatch: sample={sample_mean:.1}, theory={theoretical_mttf:.1}, error={:.1}%",
            relative_error * 100.0
        );
    }

    /// Stirling-based Gamma approximation (Lanczos, g=7) for positive real arguments.
    fn gamma_approx(x: f64) -> f64 {
        // Lanczos coefficients (g=7, n=9)
        let p = [
            0.99999999999980993,
            676.5203681218851,
            -1259.1392167224028,
            771.32342877765313,
            -176.61502916214059,
            12.507343278686905,
            -0.13857109526572012,
            9.9843695780195716e-6,
            1.5056327351493116e-7,
        ];
        let g = 7.0_f64;
        if x < 0.5 {
            std::f64::consts::PI / ((std::f64::consts::PI * x).sin() * gamma_approx(1.0 - x))
        } else {
            let x = x - 1.0;
            let mut a = p[0];
            for i in 1..9 {
                a += p[i] / (x + i as f64);
            }
            let t = x + g + 0.5;
            (2.0 * std::f64::consts::PI).sqrt() * t.powf(x + 0.5) * (-t).exp() * a
        }
    }

    // ── ZoneOutage test ─────────────────────────────────────────────

    /// Verify that ZoneOutage applies latency to agents in the selected spatial strip
    /// at the scheduled tick, and agents recover after the duration.
    #[test]
    fn zone_outage_applies_latency_and_recovers() {
        let agents: Vec<SimAgent> = (0..8)
            .map(|i| SimAgent::new(IVec2::new(i % 8, 3))) // y=3 is Pickup zone
            .collect();
        let schedule = FaultSchedule {
            events: vec![crate::fault::scenario::ScheduledEvent {
                tick: 5,
                action: ScheduledAction::ZoneLatency { duration: 10 },
                fired: false,
            }],
            initialized: true,
        };
        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents,
            test_solver(),
            test_rng(),
            disabled_fault_config(),
            schedule,
        );

        // Run to tick 4 — no latency yet
        for _ in 0..4 {
            let r = runner.tick(&RandomScheduler, &ClosestQueuePolicy);
            assert!(r.fault_events.is_empty());
        }

        // Tick 5 — zone outage fires
        let r = runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        let latency_events: Vec<_> =
            r.fault_events.iter().filter(|e| e.fault_type == FaultType::Latency).collect();
        assert!(!latency_events.is_empty(), "ZoneOutage should inject latency at tick 5");

        // Check that affected agents have latency_remaining set
        let affected_count =
            runner.agents.iter().filter(|a| a.alive && a.latency_remaining > 0).count();
        assert!(affected_count > 0, "some agents should have latency after ZoneOutage");

        // Run for duration ticks — latency should expire
        for _ in 0..10 {
            runner.tick(&RandomScheduler, &ClosestQueuePolicy);
        }
        let still_latent =
            runner.agents.iter().filter(|a| a.alive && a.latency_remaining > 0).count();
        assert_eq!(still_latent, 0, "all agents should recover after latency duration");
    }

    // ── Pre-sampled Weibull determinism across reset ────────────────

    #[test]
    fn weibull_presampled_ticks_deterministic_after_reset() {
        let agents = vec![SimAgent::new(IVec2::new(1, 1)), SimAgent::new(IVec2::new(3, 3))];
        let mut runner = SimulationRunner::new(
            test_grid(),
            test_zones(),
            agents.clone(),
            test_solver(),
            test_rng(),
            FaultConfig {
                enabled: true,
                weibull_enabled: true,
                weibull_beta: 2.5,
                weibull_eta: 500.0,
                intermittent_enabled: false,
                ..Default::default()
            },
            default_schedule(),
        );
        let ticks_before = runner.weibull_failure_ticks().to_vec();

        runner.reset();
        let ticks_after = runner.weibull_failure_ticks().to_vec();

        assert_eq!(ticks_before, ticks_after, "failure ticks must be identical after reset");
    }

    // ── F1: Weibull quantile cross-validation ─────────────────────────

    /// Verify that Weibull inverse CDF produces correct distribution quantiles.
    /// For Weibull(beta=2.5, eta=500): median ~381, P10 ~112, P90 ~660.
    /// 100,000 samples; empirical quantiles must be within 3% of theory.
    #[test]
    fn weibull_quantiles_match_theory() {
        use crate::core::seed::SeededRng;

        let n = 100_000;
        let beta = 2.5_f32;
        let eta = 500.0_f32;
        let mut rng = SeededRng::new(42);
        let ticks = SimulationRunner::sample_weibull_ticks(n, beta, eta, &mut rng);

        let mut sorted: Vec<u32> = ticks;
        sorted.sort();

        let p10 = sorted[n / 10] as f64;
        let median = sorted[n / 2] as f64;
        let p90 = sorted[9 * n / 10] as f64;

        // Theoretical values: t = eta * (-ln(1-p))^(1/beta)
        // equivalently for survival function: t = eta * (-ln(U))^(1/beta)
        let beta_f64 = beta as f64;
        let eta_f64 = eta as f64;
        let t_median = eta_f64 * (-0.5_f64.ln()).powf(1.0 / beta_f64);
        let t_p10 = eta_f64 * (-0.9_f64.ln()).powf(1.0 / beta_f64);
        let t_p90 = eta_f64 * (-0.1_f64.ln()).powf(1.0 / beta_f64);

        assert!(
            (median - t_median).abs() / t_median < 0.03,
            "median: empirical={median:.1} theoretical={t_median:.1}"
        );
        assert!(
            (p10 - t_p10).abs() / t_p10 < 0.03,
            "P10: empirical={p10:.1} theoretical={t_p10:.1}"
        );
        assert!(
            (p90 - t_p90).abs() / t_p90 < 0.03,
            "P90: empirical={p90:.1} theoretical={t_p90:.1}"
        );
    }

    // ── F4: Weibull MTTF for all WearHeatRate presets ─────────────────

    /// Verify that all 3 WearHeatRate presets produce MTTF values matching
    /// theoretical Gamma function expectations.
    /// Low (2.0, 900): MTTF ~ 798, Medium (2.5, 500): MTTF ~ 444, High (3.5, 150): MTTF ~ 137.
    #[test]
    fn weibull_mttf_all_presets() {
        use crate::core::seed::SeededRng;
        use crate::fault::scenario::WearHeatRate;

        let n = 50_000;

        for rate in &[WearHeatRate::Low, WearHeatRate::Medium, WearHeatRate::High] {
            let (beta, eta) = rate.weibull_params();

            let mut rng = SeededRng::new(12345);
            let ticks = SimulationRunner::sample_weibull_ticks(n, beta, eta, &mut rng);

            let sample_mean: f64 = ticks.iter().map(|&t| t as f64).sum::<f64>() / n as f64;

            // Theoretical MTTF = eta * Gamma(1 + 1/beta)
            let gamma_val = gamma_approx(1.0 + 1.0 / beta as f64);
            let theoretical_mttf = eta as f64 * gamma_val;

            let relative_error = ((sample_mean - theoretical_mttf) / theoretical_mttf).abs();
            assert!(
                relative_error < 0.05,
                "WearHeatRate::{:?} MTTF mismatch: sample={sample_mean:.1}, theory={theoretical_mttf:.1}, error={:.1}%",
                rate,
                relative_error * 100.0
            );
        }
    }
}
