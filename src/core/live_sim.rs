//! LiveSim — Bevy resource wrapping SimulationRunner + AnalysisEngine.
//!
//! `drive_simulation` runs one tick per FixedUpdate, replacing the old
//! chain of ECS systems (tick_agents → recycle_goals → lifelong_replan
//! + all fault systems).
//!
//! `sync_runner_to_ecs` writes runner state back to ECS components so
//! existing render, bridge, and analysis systems continue working during
//! the transitional period.

use bevy::prelude::*;

use crate::analysis::engine::AnalysisEngine;
use crate::core::agent::{AgentIndex, AgentRegistry, LastAction, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::SimulationRunner;
use crate::core::seed::SeededRng;
use crate::core::state::{ResumeTarget, SimState, SimulationConfig, StepMode};
use crate::core::task::{ActiveScheduler, LifelongConfig};
use crate::fault::breakdown::{Dead, FaultEvent, LatencyFault};
use crate::fault::heat::HeatState;

// ---------------------------------------------------------------------------
// Resource
// ---------------------------------------------------------------------------

/// The live simulation — single source of truth for all simulation state.
///
/// Created during `begin_loading`, consumed during `SimState::Running`.
/// Bevy systems read this to render and bridge to JS.
#[derive(Resource)]
pub struct LiveSim {
    pub runner: SimulationRunner,
    pub analysis: AnalysisEngine,
}

impl LiveSim {
    pub fn new(runner: SimulationRunner, tick_capacity: usize) -> Self {
        let mut analysis = AnalysisEngine::new(tick_capacity);
        analysis.record_positions = true; // live sim needs positions for parity verification
        Self { runner, analysis }
    }
}

// ---------------------------------------------------------------------------
// drive_simulation — replaces the FixedUpdate chain
// ---------------------------------------------------------------------------

/// Single system that advances the simulation by one tick.
///
/// Replaces: `tick_agents → recycle_goals → lifelong_replan` +
/// `execute_fault_schedule → accumulate_heat → detect_faults →
/// replan_after_fault → apply_latency`.
pub fn drive_simulation(
    mut sim: ResMut<LiveSim>,
    scheduler: Res<ActiveScheduler>,
    queue_policy: Res<ActiveQueuePolicy>,
    mut next_state: ResMut<NextState<SimState>>,
    mut step_mode: ResMut<StepMode>,
    config: Res<SimulationConfig>,
    mut resume_target: ResMut<ResumeTarget>,
    mut fixed_time: ResMut<Time<Fixed>>,
    mut fault_events: MessageWriter<FaultEvent>,
    agent_registry: Res<AgentRegistry>,
) {
    let mut result = sim.runner.tick(scheduler.scheduler(), queue_policy.policy());

    // Forward runner fault events to ECS BEFORE record_tick (which takes ownership via mem::take)
    for fe in &result.fault_events {
        if let Some(entity) = agent_registry.get_entity(AgentIndex(fe.agent_index)) {
            fault_events.write(FaultEvent {
                entity,
                fault_type: fe.fault_type,
                source: fe.source,
                tick: fe.tick,
                position: fe.position,
                paths_invalidated: fe.paths_invalidated,
            });
        }
    }

    // Split borrow: analysis borrows runner immutably via the AnalysisEngine
    // method that takes &SimulationRunner. We need to reborrow after tick().
    let LiveSim { ref runner, ref mut analysis } = *sim;
    analysis.record_tick(runner, &mut result);

    // ── State transitions ──────────────────────────────────────────────

    // Fast-forward: if resuming from a rewound tick, restore normal speed
    if let Some(target) = resume_target.target_tick
        && sim.runner.tick >= target
    {
        resume_target.target_tick = None;
        *fixed_time = Time::<Fixed>::from_hz(config.tick_hz);
        return;
    }

    if step_mode.pending {
        step_mode.pending = false;
        next_state.set(SimState::Paused);
        return;
    }

    // Auto-finish at duration
    if sim.runner.tick >= config.duration {
        next_state.set(SimState::Finished);
        return;
    }

    // Legacy max_ticks (used by export/tests)
    if let Some(max) = config.max_ticks
        && sim.runner.tick >= max
    {
        next_state.set(SimState::Finished);
    }

    // In lifelong mode, agents always get new goals; no finish trigger here.
}

// ---------------------------------------------------------------------------
// sync_runner_to_ecs — transitional bridge to existing ECS consumers
// ---------------------------------------------------------------------------

/// Bridges `SimulationRunner` state to ECS components so render, bridge,
/// and analysis systems can read agent positions, heat, dead/latency status,
/// and per-tick counters without depending on `LiveSim` directly.
pub fn sync_runner_to_ecs(
    sim: Res<LiveSim>,
    registry: Res<AgentRegistry>,
    mut agents: Query<(&AgentIndex, &mut LogicalAgent, Option<&mut HeatState>)>,
    mut config: ResMut<SimulationConfig>,
    mut lifelong: ResMut<LifelongConfig>,
    mut grid: ResMut<GridMap>,
    mut ecs_rng: ResMut<SeededRng>,
    mut ecs_solver: ResMut<crate::solver::ActiveSolver>,
    mut commands: Commands,
    dead_query: Query<Entity, With<Dead>>,
    latency_query: Query<Entity, With<LatencyFault>>,
) {
    let runner = &sim.runner;

    // Sync tick counter
    config.tick = runner.tick;

    // Sync lifelong state
    lifelong.tasks_completed = runner.tasks_completed;
    // Only clone completion_ticks if a task was completed this tick
    if runner.completion_ticks().back().copied() == Some(runner.tick) {
        lifelong.set_completion_ticks(runner.completion_ticks().clone());
    }

    // Sync RNG state (for tick history snapshots)
    ecs_rng.rng.set_word_pos(runner.rng().rng.get_word_pos());

    // Sync solver priorities (for tick history snapshots)
    let priorities = runner.solver().save_priorities();
    ecs_solver.restore_priorities(&priorities);

    // Sync grid (faults may add/remove obstacles via temp blockages)
    // Only clone if obstacle count differs (cheap check to avoid unnecessary clones)
    if runner.grid().obstacle_count() != grid.obstacle_count() {
        *grid = runner.grid().clone();
    }

    // Sync per-agent state
    for (idx, mut logical, mut heat_opt) in &mut agents {
        let i = idx.0;
        if i >= runner.agents.len() {
            continue;
        }
        let sa = &runner.agents[i];

        // Position + goal + path length + task_leg (only clone if changed)
        logical.current_pos = sa.pos;
        logical.goal_pos = sa.goal;
        logical.path_length = sa.planned_path.len();
        if logical.task_leg != sa.task_leg {
            logical.task_leg = sa.task_leg.clone();
        }

        // Heat
        if let Some(ref mut heat) = heat_opt {
            heat.heat = sa.heat;
        }

        let entity = match registry.get_entity(AgentIndex(i)) {
            Some(e) => e,
            None => continue,
        };

        // LastAction (only insert if changed — avoids ECS command overhead)
        commands.entity(entity).insert(LastAction(sa.last_action));

        // Dead state
        if !sa.alive && dead_query.get(entity).is_err() {
            commands.entity(entity).insert(Dead);
        }

        // Latency state
        if sa.latency_remaining > 0 {
            if latency_query.get(entity).is_err() {
                commands.entity(entity).insert(LatencyFault { remaining: sa.latency_remaining });
            }
        } else if latency_query.get(entity).is_ok() {
            commands.entity(entity).remove::<LatencyFault>();
        }
    }
}
