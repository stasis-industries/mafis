#[cfg(not(any(test, feature = "headless")))]
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

#[cfg(not(any(test, feature = "headless")))]
use crate::constants;
#[cfg(not(any(test, feature = "headless")))]
use crate::core::agent::{AgentIndex, AgentRegistry, LogicalAgent};
#[cfg(not(any(test, feature = "headless")))]
use crate::core::grid::GridMap;
#[cfg(not(any(test, feature = "headless")))]
use crate::core::state::SimulationConfig;
#[cfg(not(any(test, feature = "headless")))]
use crate::render::environment::{ObstacleMarker, grid_to_world};

#[cfg(not(any(test, feature = "headless")))]
use super::breakdown::{Dead, FaultEvent, LatencyFault};
use super::config::FaultSource;
#[cfg(not(any(test, feature = "headless")))]
use super::config::FaultType;
#[cfg(not(any(test, feature = "headless")))]
use super::heat::HeatState;

/// Manual fault injection commands from the UI / bridge.
#[derive(Message)]
pub enum ManualFaultCommand {
    /// Kill an agent by index — tags Dead, places obstacle.
    KillAgent(usize),
    /// Place a permanent obstacle at a grid cell.
    PlaceObstacle(IVec2),
    /// Inject latency on an agent — forces Wait for `duration` ticks.
    InjectLatency { agent_id: usize, duration: u32 },
    /// Kill agent from a scheduled scenario (uses FaultSource::Scheduled).
    KillAgentScheduled(usize),
    /// Inject latency from a scheduled scenario.
    InjectLatencyScheduled { agent_id: usize, duration: u32 },
}

// ---------------------------------------------------------------------------
// ManualFaultLog — records every manual fault for rewind replay
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ManualFaultAction {
    KillAgent { agent_index: usize, pos: IVec2 },
    PlaceObstacle(IVec2),
    InjectLatency { agent_id: usize, duration: u32 },
}

#[derive(Debug, Clone)]
pub struct ManualFaultEntry {
    pub tick: u64,
    pub action: ManualFaultAction,
    pub source: FaultSource,
}

#[derive(Resource, Debug, Default)]
pub struct ManualFaultLog {
    pub entries: Vec<ManualFaultEntry>,
    /// Index of the next entry to replay after rewind. When `Some`, entries at
    /// indices >= this value are candidates for replay at their original tick.
    /// Set by `restore_world_state`, consumed by `replay_manual_faults`.
    pub replay_from: Option<usize>,
}

impl ManualFaultLog {
    /// Insert an entry maintaining chronological (tick-sorted) order.
    /// Entries placed during Replay at earlier ticks must not break the
    /// sorted invariant that `restore_world_state` and `replay_manual_faults`
    /// depend on (both use `break` on tick > target).
    pub fn insert_sorted(&mut self, entry: ManualFaultEntry) {
        let pos = self.entries.partition_point(|e| e.tick <= entry.tick);
        self.entries.insert(pos, entry);
    }

    pub fn truncate_after_tick(&mut self, tick: u64) {
        self.entries.retain(|e| e.tick <= tick);
        self.replay_from = None;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.replay_from = None;
    }
}

// ---------------------------------------------------------------------------
// RewindRequest — set by bridge, consumed by apply_rewind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum RewindKind {
    /// Resume from replay: restore world state to snapshot tick, then run.
    ResumeFromTick(u64),
}

#[derive(Resource, Default, Debug)]
pub struct RewindRequest {
    pub pending: Option<RewindKind>,
}

// ---------------------------------------------------------------------------
// PendingManualFaults — Update-side buffer drained in FixedUpdate
// ---------------------------------------------------------------------------

/// Buffered `FaultEvent`s produced in `Update` by `process_manual_faults`.
///
/// `propagate_cascade` (the reader) runs in `FixedUpdate`. Writing the event
/// directly from `Update` makes the read order non-deterministic across
/// schedules. We collect raw event payloads here and emit them as proper
/// `FaultEvent` messages from a `FixedUpdate` system in `FaultSet::Schedule`.
#[cfg(not(any(test, feature = "headless")))]
#[derive(Resource, Default)]
pub struct PendingManualFaults {
    pub events: Vec<super::breakdown::FaultEvent>,
}

#[cfg(any(test, feature = "headless"))]
#[derive(Resource, Default)]
pub struct PendingManualFaults;

#[cfg(not(any(test, feature = "headless")))]
/// Drain `PendingManualFaults` collected in `Update` and emit `FaultEvent`
/// messages in `FixedUpdate`. Runs in `FaultSet::Schedule` so cascade BFS
/// readers pick them up on the same tick.
pub fn drain_pending_manual_faults(
    mut pending: ResMut<PendingManualFaults>,
    mut writer: MessageWriter<super::breakdown::FaultEvent>,
) {
    if pending.events.is_empty() {
        return;
    }
    for fe in pending.events.drain(..) {
        writer.write(fe);
    }
}

// ---------------------------------------------------------------------------
// apply_rewind system (render-dependent, excluded from test builds)
// ---------------------------------------------------------------------------

#[cfg(not(any(test, feature = "headless")))]
#[derive(SystemParam)]
pub struct RewindResources<'w> {
    grid: ResMut<'w, GridMap>,
    topology: Res<'w, crate::core::topology::ActiveTopology>,
    fault_log: ResMut<'w, ManualFaultLog>,
    fault_schedule: ResMut<'w, super::scenario::FaultSchedule>,
    tick_history: ResMut<'w, crate::analysis::history::TickHistory>,
    config: ResMut<'w, SimulationConfig>,
    solver: ResMut<'w, crate::solver::ActiveSolver>,
    lifelong: ResMut<'w, crate::core::task::LifelongConfig>,
    rng: ResMut<'w, crate::core::seed::SeededRng>,
    ui_state: Res<'w, crate::ui::controls::UiState>,
    dist_cache: ResMut<'w, crate::solver::heuristics::DistanceMapCache>,
    rewind_req: ResMut<'w, RewindRequest>,
    baseline_diff: ResMut<'w, crate::analysis::baseline::BaselineDiff>,
    baseline_store: Res<'w, crate::analysis::baseline::BaselineStore>,
}

#[cfg(not(any(test, feature = "headless")))]
/// Applies pending rewind requests. Runs in Update after BridgeSet.
pub fn apply_rewind(
    mut commands: Commands,
    mut res: RewindResources,
    mut next_state: ResMut<NextState<crate::core::state::SimState>>,
    mut agents: Query<(Entity, &mut LogicalAgent, &AgentIndex, Has<Dead>, Has<LatencyFault>)>,
    mut heat_query: Query<&mut HeatState>,
    obstacles: Query<Entity, With<ObstacleMarker>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sim: Option<ResMut<crate::core::live_sim::LiveSim>>,
) {
    let request = match res.rewind_req.pending.take() {
        Some(r) => r,
        None => return,
    };

    match request {
        RewindKind::ResumeFromTick(target_tick) => {
            let snapshot = match find_snapshot(&res.tick_history, target_tick) {
                Some(s) => s.clone(),
                None => return,
            };

            restore_world_state(
                &mut commands,
                &mut res,
                &mut agents,
                &mut heat_query,
                &obstacles,
                &mut meshes,
                &mut materials,
                &snapshot,
            );

            // Restore runner state to match ECS
            restore_runner_state(&mut sim, &res, &snapshot);

            // Also sync fault schedule on runner
            if let Some(ref mut sim) = sim {
                sim.runner.fault_schedule_mut().un_fire_after_tick(target_tick);
                sim.analysis.truncate_to_tick(snapshot.tick);
            }

            // Un-fire scheduled events so they re-fire when the simulation
            // reaches those ticks again.
            res.fault_schedule.un_fire_after_tick(target_tick);

            // Truncate snapshots beyond the rewind point — the old timeline
            // no longer represents reality (simulation will diverge due to
            // new faults). Without this, record_tick_snapshot skips new
            // recordings because it sees tick <= last snapshot tick.
            res.tick_history.truncate_after_tick(target_tick);

            // Don't truncate manual fault log — entries beyond target_tick
            // are replayed by replay_manual_faults at their original ticks.
            // restore_world_state already sets replay_from to the first
            // entry after target_tick.

            // Recompute BaselineDiff from scratch so cumulative metrics
            // (deficit_integral, surplus_integral, recovery_tick) reflect
            // only the ticks up to the rewind point. From this point on,
            // update_baseline_diff will accumulate correctly as the
            // simulation re-runs with the injected faults.
            if let Some(ref record) = res.baseline_store.record {
                if let Some(ref sim) = sim {
                    let tasks = &sim.analysis.tasks_completed_series;
                    let tp = &sim.analysis.throughput_series;
                    res.baseline_diff.recompute(record, tasks, tp);
                } else {
                    res.baseline_diff.clear();
                }
            } else {
                res.baseline_diff.clear();
            }

            res.tick_history.replay_cursor = None;

            next_state.set(crate::core::state::SimState::Running);
        }
    }
}

#[cfg(not(any(test, feature = "headless")))]
fn find_snapshot(
    history: &crate::analysis::history::TickHistory,
    tick: u64,
) -> Option<&crate::analysis::history::FullTickSnapshot> {
    let idx = history.tick_to_index(tick)?;
    history.snapshots.get(idx)
}

#[cfg(not(any(test, feature = "headless")))]
fn restore_world_state(
    commands: &mut Commands,
    res: &mut RewindResources,
    agents: &mut Query<(Entity, &mut LogicalAgent, &AgentIndex, Has<Dead>, Has<LatencyFault>)>,
    heat_query: &mut Query<&mut HeatState>,
    obstacles: &Query<Entity, With<ObstacleMarker>>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    snapshot: &crate::analysis::history::FullTickSnapshot,
) {
    use std::collections::{HashMap, HashSet};

    let target_tick = snapshot.tick;

    // 1. Rebuild grid from topology (clean slate)
    let topo_output = res.topology.topology().generate(res.ui_state.seed);
    *res.grid = topo_output.grid;

    // 2. Replay manual fault log entries that modify the grid (tick <= snapshot tick)
    //    Track fault-placed obstacle positions so they get the dark color, not topology grey.
    let mut fault_obstacle_cells: HashSet<IVec2> = HashSet::new();
    for entry in &res.fault_log.entries {
        if entry.tick > target_tick {
            break; // entries are in chronological order
        }
        match &entry.action {
            ManualFaultAction::KillAgent { pos, .. } => {
                res.grid.set_obstacle(*pos);
                fault_obstacle_cells.insert(*pos);
            }
            ManualFaultAction::PlaceObstacle(cell) => {
                res.grid.set_obstacle(*cell);
                fault_obstacle_cells.insert(*cell);
            }
            // Latency is transient — skip for grid rebuild
            _ => {}
        }
    }

    // 3. Restore agent states from snapshot
    let snap_map: HashMap<usize, &crate::analysis::history::AgentSnapshot> =
        snapshot.agents.iter().map(|s| (s.index, s)).collect();

    for (entity, mut agent, idx, has_dead, has_latency) in agents.iter_mut() {
        if let Some(snap) = snap_map.get(&idx.0) {
            agent.current_pos = snap.pos;
            agent.goal_pos = snap.goal;
            agent.task_leg = snap.reconstruct_task_leg();
            // Restore the planned path so the first resumed tick executes the
            // same moves as the original run (not a forced Wait due to empty plan).
            agent.planned_path.clear();
            agent.planned_path.extend(
                snap.planned_actions.iter().map(|&b| crate::core::action::Action::from_u8(b)),
            );

            // Restore heat state (determines fault trigger thresholds)
            if let Ok(mut heat_state) = heat_query.get_mut(entity) {
                heat_state.heat = snap.heat;
                heat_state.total_moves = 0;
            }

            // Sync Dead component with snapshot
            if snap.is_dead && !has_dead {
                commands.entity(entity).insert(Dead);
            } else if !snap.is_dead && has_dead {
                commands.entity(entity).remove::<Dead>();
            }
        }

        // Remove all latency faults (transient, can't reconstruct remaining duration)
        if has_latency {
            commands.entity(entity).remove::<LatencyFault>();
        }
    }

    // 4. Despawn all obstacle visuals and respawn from current grid state.
    //    Topology obstacles get light grey; fault-placed obstacles (dead agents,
    //    manual walls) get dark brown so they remain visually distinct.
    for entity in obstacles.iter() {
        commands.entity(entity).despawn();
    }
    {
        use crate::render::environment::{ObstacleMarker, grid_to_world};
        const CELL_SIZE: f32 = 1.0;
        const OBSTACLE_HEIGHT: f32 = 0.45;

        let topo_mesh = meshes.add(Cuboid::new(CELL_SIZE * 0.9, OBSTACLE_HEIGHT, CELL_SIZE * 0.9));
        let topo_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.47, 0.52),
            perceptual_roughness: 0.55,
            metallic: 0.15,
            ..default()
        });
        let fault_mesh = meshes.add(Cuboid::new(0.9, 0.6, 0.9));
        let fault_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.20, 0.18),
            perceptual_roughness: 0.7,
            ..default()
        });

        for &pos in res.grid.obstacles() {
            let world = grid_to_world(pos);
            if fault_obstacle_cells.contains(&pos) {
                commands.spawn((
                    Mesh3d(fault_mesh.clone()),
                    MeshMaterial3d(fault_mat.clone()),
                    Transform::from_xyz(world.x, 0.3, world.z),
                    ObstacleMarker,
                ));
            } else {
                commands.spawn((
                    Mesh3d(topo_mesh.clone()),
                    MeshMaterial3d(topo_mat.clone()),
                    Transform::from_xyz(world.x, OBSTACLE_HEIGHT * 0.5, world.z),
                    ObstacleMarker,
                ));
            }
        }
    }

    // 6. Reset simulation state
    res.config.tick = target_tick;

    // Restore exact RNG state: reseed to original, then fast-forward the stream
    // to the exact word position recorded at snapshot time. This guarantees
    // identical random sequences from this point forward (true determinism).
    let original_seed = res.rng.seed();
    res.rng.reseed(original_seed);
    res.rng.rng.set_word_pos(snapshot.rng_word_pos);

    // Restore solver priority state from snapshot instead of resetting.
    // reset() would clear accumulated priorities, causing different agent ordering
    // on resumed simulation (non-determinism). Priority restore preserves exact state.
    if !snapshot.solver_priorities.is_empty() {
        res.solver.restore_priorities(&snapshot.solver_priorities);
    } else {
        // Fallback for snapshots taken before priority saving was added
        res.solver.reset();
    }
    res.dist_cache.clear();

    // Restore solver-specific planning caches that `reset()` / `restore_priorities()`
    // did not recover. Token Passing rebuilds its per-agent token paths (and,
    // implicitly, the MasterConstraintIndex on the next step) from each agent's
    // restored planned_actions. Without this, TP replay diverges because the
    // constraint index starts empty while the original run had multi-step plans
    // active at this tick. PIBT and RHCR-PBS default to no-op.
    {
        use crate::core::action::Action;
        use crate::solver::lifelong::AgentRestoreState;
        let actions_per_agent: Vec<Vec<Action>> = snapshot
            .agents
            .iter()
            .map(|s| s.planned_actions.iter().map(|&b| Action::from_u8(b)).collect())
            .collect();
        let restore_data: Vec<AgentRestoreState> = snapshot
            .agents
            .iter()
            .zip(actions_per_agent.iter())
            .map(|(s, actions)| AgentRestoreState {
                index: s.index,
                pos: s.pos,
                goal: Some(s.goal),
                task_leg: s.reconstruct_task_leg(),
                planned_actions: actions.as_slice(),
            })
            .collect();
        res.solver.restore_state(&restore_data);
    }

    // Restore lifelong task count + completion_ticks window for correct throughput
    res.lifelong.restore_from_snapshot(
        snapshot.lifelong_tasks_completed,
        snapshot.completion_ticks.clone(),
    );

    // Set up replay cursor: find the first fault log entry after the snapshot tick.
    // The replay_manual_faults system will re-fire these at their original ticks.
    let replay_idx = res.fault_log.entries.iter().position(|e| e.tick > target_tick);
    res.fault_log.replay_from = replay_idx;
}

/// Restore `SimulationRunner` state to match the snapshot.
/// Must be called after `restore_world_state` so `res.grid` is already rebuilt.
#[cfg(not(any(test, feature = "headless")))]
fn restore_runner_state(
    sim: &mut Option<ResMut<crate::core::live_sim::LiveSim>>,
    res: &RewindResources,
    snapshot: &crate::analysis::history::FullTickSnapshot,
) {
    let sim = match sim.as_mut() {
        Some(s) => s,
        None => return,
    };
    let runner = &mut sim.runner;

    // Tick
    runner.tick = snapshot.tick;

    // Grid (already rebuilt by restore_world_state → use ECS grid)
    *runner.grid_mut() = res.grid.clone();

    // Agents
    for snap in &snapshot.agents {
        if snap.index < runner.agents.len() {
            let agent = &mut runner.agents[snap.index];
            agent.pos = snap.pos;
            agent.goal = snap.goal;
            agent.heat = snap.heat;
            agent.alive = !snap.is_dead;
            agent.task_leg = snap.reconstruct_task_leg();
            agent.planned_path.clear();
            agent.planned_path.extend(
                snap.planned_actions.iter().map(|&b| crate::core::action::Action::from_u8(b)),
            );
            agent.latency_remaining = 0; // transient, can't reconstruct duration
            agent.last_action = crate::core::action::Action::Wait;
            agent.operational_age = snap.operational_age;
            // Restore intermittent fault sampling state so re-running forward
            // doesn't re-initialize Phase 1 and double-fire events.
            agent.next_fault_tick =
                snapshot.intermittent_next_fault_tick.get(snap.index).copied().flatten();
        }
    }

    // RNG — reseed to original then fast-forward to snapshot word position
    let seed = runner.rng().seed();
    runner.rng_mut().reseed(seed);
    runner.rng_mut().rng.set_word_pos(snapshot.rng_word_pos);

    // Fault RNG — same pattern: reseed to original then fast-forward
    let fault_seed = runner.fault_rng().seed();
    runner.fault_rng_mut().reseed(fault_seed);
    runner.fault_rng_mut().rng.set_word_pos(snapshot.fault_rng_word_pos);

    // Solver — always reset first to clear transient state (congestion_streak,
    // plan_buffer, ticks_since_replan, etc.), then restore priorities if available.
    // Without the unconditional reset, RHCR's transient state persists across rewind.
    runner.solver_mut().reset();
    if !snapshot.solver_priorities.is_empty() {
        runner.solver_mut().restore_priorities(&snapshot.solver_priorities);
    }

    // Rebuild solver-specific planning caches (see comment on the ECS path).
    {
        use crate::core::action::Action;
        use crate::solver::lifelong::AgentRestoreState;
        let actions_per_agent: Vec<Vec<Action>> = snapshot
            .agents
            .iter()
            .map(|s| s.planned_actions.iter().map(|&b| Action::from_u8(b)).collect())
            .collect();
        let restore_data: Vec<AgentRestoreState> = snapshot
            .agents
            .iter()
            .zip(actions_per_agent.iter())
            .map(|(s, actions)| AgentRestoreState {
                index: s.index,
                pos: s.pos,
                goal: Some(s.goal),
                task_leg: s.reconstruct_task_leg(),
                planned_actions: actions.as_slice(),
            })
            .collect();
        runner.solver_mut().restore_state(&restore_data);
    }

    // Completion state
    runner.restore_completion_state(
        snapshot.lifelong_tasks_completed,
        snapshot.completion_ticks.clone(),
    );

    // Clear transient state (temp blockages, command queue, dist cache)
    runner.clear_transient_state();
}

// ---------------------------------------------------------------------------
// replay_manual_faults — re-fires logged faults after rewind (FixedUpdate)
// ---------------------------------------------------------------------------

/// Replays manual fault log entries at their original ticks after a rewind.
/// Runs in FixedUpdate so it catches every tick. Directly modifies grid/entities
/// and the runner without going through the ManualFaultCommand message pipeline
/// (avoids re-logging). Does NOT re-register events in fault_metrics (the originals
/// are still there).
#[cfg(not(any(test, feature = "headless")))]
pub fn replay_manual_faults(
    mut commands: Commands,
    mut fault_log: ResMut<ManualFaultLog>,
    agent_registry: Res<AgentRegistry>,
    agents_query: Query<&LogicalAgent>,
    mut grid: ResMut<GridMap>,
    sim_config: Res<SimulationConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sim: Option<ResMut<crate::core::live_sim::LiveSim>>,
) {
    let replay_idx = match fault_log.replay_from {
        Some(idx) => idx,
        None => return,
    };

    if replay_idx >= fault_log.entries.len() {
        fault_log.replay_from = None;
        return;
    }

    let tick = sim_config.tick;

    // Lazily-created shared handles for obstacle visuals
    let mut obstacle_mesh: Option<Handle<Mesh>> = None;
    let mut obstacle_mat: Option<Handle<StandardMaterial>> = None;

    let mut new_replay_from = replay_idx;
    let entries_len = fault_log.entries.len();

    // Process all entries at the current tick
    for i in replay_idx..entries_len {
        let entry = &fault_log.entries[i];
        if entry.tick > tick {
            break;
        }
        if entry.tick < tick {
            // Already past — advance cursor
            new_replay_from = i + 1;
            continue;
        }

        // Skip scheduled-source entries: these are re-fired by the FaultSchedule
        // (via un_fire_after_tick → execute_fault_schedule → ManualFaultCommand).
        // Replaying them here would cause double-fire.
        if entry.source == FaultSource::Scheduled {
            new_replay_from = i + 1;
            continue;
        }

        // Entry is at current tick — replay it
        match &entry.action {
            ManualFaultAction::KillAgent { agent_index, pos } => {
                if let Some(entity) = agent_registry.get_entity(AgentIndex(*agent_index)) {
                    // Use the agent's current position — after rewind the agent
                    // may have taken a different path and won't be at the stored pos.
                    let kill_pos = agents_query.get(entity).map(|a| a.current_pos).unwrap_or(*pos);

                    commands.entity(entity).insert(Dead);
                    grid.set_obstacle(kill_pos);

                    // No obstacle visual — the Dead component changes the agent's
                    // color to dark red via the rendering system. Spawning a brown
                    // obstacle cube on top makes KillAgent visually identical to
                    // PlaceObstacle, which confuses the user.

                    // Also apply to runner
                    if let Some(ref mut sim) = sim
                        && *agent_index < sim.runner.agents.len()
                    {
                        sim.runner.agents[*agent_index].alive = false;
                        sim.runner.agents[*agent_index].planned_path.clear();
                        sim.runner.grid_mut().set_obstacle(kill_pos);
                    }
                }
            }
            ManualFaultAction::PlaceObstacle(cell) => {
                if grid.is_in_bounds(*cell) && grid.is_walkable(*cell) {
                    grid.set_obstacle(*cell);

                    let m =
                        obstacle_mesh.get_or_insert_with(|| meshes.add(Cuboid::new(0.9, 0.6, 0.9)));
                    let mt = obstacle_mat.get_or_insert_with(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::srgb(0.35, 0.20, 0.18),
                            perceptual_roughness: 0.7,
                            ..default()
                        })
                    });
                    let world = grid_to_world(*cell);
                    commands.spawn((
                        Mesh3d(m.clone()),
                        MeshMaterial3d(mt.clone()),
                        Transform::from_xyz(world.x, 0.3, world.z),
                        ObstacleMarker,
                    ));

                    // Also apply to runner
                    if let Some(ref mut sim) = sim {
                        sim.runner.grid_mut().set_obstacle(*cell);
                    }
                }
            }
            ManualFaultAction::InjectLatency { agent_id, duration } => {
                if let Some(entity) = agent_registry.get_entity(AgentIndex(*agent_id))
                    && agents_query.get(entity).is_ok()
                {
                    commands.entity(entity).insert(LatencyFault { remaining: *duration });
                }
                // Also apply to runner
                if let Some(ref mut sim) = sim
                    && *agent_id < sim.runner.agents.len()
                {
                    sim.runner.agents[*agent_id].latency_remaining = *duration;
                }
            }
        }
        new_replay_from = i + 1;
    }

    // Advance cursor
    if new_replay_from >= entries_len {
        fault_log.replay_from = None;
    } else {
        fault_log.replay_from = Some(new_replay_from);
    }
}

/// Processes manual fault commands. Runs in Update (works when Paused).
/// Applies faults to both ECS (immediate visual) and runner (persistence).
///
/// `FaultEvent`s are buffered in `PendingManualFaults` here and emitted in
/// `FixedUpdate` by `drain_pending_manual_faults` so the cascade BFS reader
/// picks them up on the same schedule.
#[cfg(not(any(test, feature = "headless")))]
#[allow(clippy::too_many_arguments)]
pub fn process_manual_faults(
    mut commands: Commands,
    mut manual_cmds: MessageReader<ManualFaultCommand>,
    mut pending_faults: ResMut<PendingManualFaults>,
    agent_registry: Res<AgentRegistry>,
    agents_query: Query<&LogicalAgent>,
    mut grid: ResMut<GridMap>,
    sim_config: Res<SimulationConfig>,
    mut fault_metrics: ResMut<crate::analysis::fault_metrics::FaultMetrics>,
    mut fault_log: ResMut<ManualFaultLog>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sim: Option<ResMut<crate::core::live_sim::LiveSim>>,
    mut tick_history: ResMut<crate::analysis::history::TickHistory>,
) {
    // Lazily-created shared handles for manual obstacle visuals
    let mut obstacle_mesh: Option<Handle<Mesh>> = None;
    let mut obstacle_mat: Option<Handle<StandardMaterial>> = None;

    let spawn_obstacle_visual = |commands: &mut Commands,
                                 meshes: &mut Assets<Mesh>,
                                 materials: &mut Assets<StandardMaterial>,
                                 mesh: &mut Option<Handle<Mesh>>,
                                 mat: &mut Option<Handle<StandardMaterial>>,
                                 cell: IVec2| {
        let m = mesh.get_or_insert_with(|| meshes.add(Cuboid::new(0.9, 0.6, 0.9)));
        let mt = mat.get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.35, 0.20, 0.18),
                perceptual_roughness: 0.7,
                ..default()
            })
        });
        let world = grid_to_world(cell);
        commands.spawn((
            Mesh3d(m.clone()),
            MeshMaterial3d(mt.clone()),
            Transform::from_xyz(world.x, 0.3, world.z),
            ObstacleMarker,
        ));
    };

    // Check replay_cursor instead of State<SimState> — Bevy state transitions
    // are deferred to the next frame, but replay_cursor is set immediately by
    // SeekToTick in the bridge. When seek + fault arrive in the same JS polling
    // batch, the state is still Finished/Paused but the cursor is already set.
    let is_replay = tick_history.replay_cursor.is_some();
    let effective_tick = if is_replay {
        tick_history.current_snapshot().map(|s| s.tick).unwrap_or(sim_config.tick)
    } else {
        sim_config.tick
    };

    // Track whether we've already applied causality truncation this frame
    // (idempotent, but avoids redundant work for multiple commands).
    let mut causality_applied = false;

    // Alive count for propagation rate denominator
    let alive_count = sim.as_ref().map_or(fault_metrics.initial_agent_count, |s| {
        s.runner.agents.iter().filter(|a| a.alive).count() as u32
    });

    for cmd in manual_cmds.read() {
        match cmd {
            ManualFaultCommand::KillAgent(id) | ManualFaultCommand::KillAgentScheduled(id) => {
                let source = if matches!(cmd, ManualFaultCommand::KillAgentScheduled(_)) {
                    FaultSource::Scheduled
                } else {
                    FaultSource::Manual
                };
                if let Some(entity) = agent_registry.get_entity(AgentIndex(*id))
                    && let Ok(agent) = agents_query.get(entity)
                {
                    // During replay, ECS current_pos is stale (from last running tick).
                    // Use snapshot position which matches what the user sees.
                    let pos = if is_replay {
                        tick_history
                            .current_snapshot()
                            .and_then(|snap| snap.agents.iter().find(|a| a.index == *id))
                            .map(|a| a.pos)
                            .unwrap_or(agent.current_pos)
                    } else {
                        agent.current_pos
                    };
                    commands.entity(entity).insert(Dead);
                    grid.set_obstacle(pos);
                    pending_faults.events.push(FaultEvent {
                        entity,
                        fault_type: FaultType::Breakdown,
                        source,
                        tick: effective_tick,
                        position: pos,
                        paths_invalidated: 0, // manual faults: cascade handled by BFS
                    });
                    fault_metrics.register_manual_event(
                        effective_tick,
                        FaultType::Breakdown,
                        source,
                        pos,
                        alive_count,
                    );
                    // Causality: changing the past invalidates the future.
                    if is_replay && !causality_applied {
                        fault_log.truncate_after_tick(effective_tick);
                        tick_history.truncate_after_tick(effective_tick);
                        if let Some(ref mut s) = sim {
                            s.analysis.truncate_to_tick(effective_tick);
                        }
                        causality_applied = true;
                    }
                    fault_log.insert_sorted(ManualFaultEntry {
                        tick: effective_tick,
                        action: ManualFaultAction::KillAgent { agent_index: *id, pos },
                        source,
                    });
                    // Also apply to runner so it persists through sync_runner_to_ecs
                    if let Some(ref mut sim) = sim
                        && *id < sim.runner.agents.len()
                    {
                        sim.runner.agents[*id].alive = false;
                        sim.runner.agents[*id].planned_path.clear();
                        sim.runner.grid_mut().set_obstacle(pos);
                    }
                }
            }
            ManualFaultCommand::PlaceObstacle(cell) => {
                if grid.is_in_bounds(*cell) && grid.is_walkable(*cell) {
                    grid.set_obstacle(*cell);
                    spawn_obstacle_visual(
                        &mut commands,
                        &mut meshes,
                        &mut materials,
                        &mut obstacle_mesh,
                        &mut obstacle_mat,
                        *cell,
                    );
                    fault_metrics.register_manual_event(
                        effective_tick,
                        FaultType::Breakdown,
                        FaultSource::Manual,
                        *cell,
                        alive_count,
                    );
                    if is_replay && !causality_applied {
                        fault_log.truncate_after_tick(effective_tick);
                        tick_history.truncate_after_tick(effective_tick);
                        if let Some(ref mut s) = sim {
                            s.analysis.truncate_to_tick(effective_tick);
                        }
                        causality_applied = true;
                    }
                    fault_log.insert_sorted(ManualFaultEntry {
                        tick: effective_tick,
                        action: ManualFaultAction::PlaceObstacle(*cell),
                        source: FaultSource::Manual,
                    });
                    // Also apply to runner
                    if let Some(ref mut sim) = sim {
                        sim.runner.grid_mut().set_obstacle(*cell);
                    }
                }
            }
            ManualFaultCommand::InjectLatency { agent_id, duration }
            | ManualFaultCommand::InjectLatencyScheduled { agent_id, duration } => {
                let source = if matches!(cmd, ManualFaultCommand::InjectLatencyScheduled { .. }) {
                    FaultSource::Scheduled
                } else {
                    FaultSource::Manual
                };
                if let Some(entity) = agent_registry.get_entity(AgentIndex(*agent_id))
                    && agents_query.get(entity).is_ok()
                {
                    let dur = if *duration == 0 {
                        constants::DEFAULT_LATENCY_DURATION
                    } else {
                        *duration
                    };
                    commands.entity(entity).insert(LatencyFault { remaining: dur });
                    if let Ok(agent) = agents_query.get(entity) {
                        let pos = agent.current_pos;
                        pending_faults.events.push(FaultEvent {
                            entity,
                            fault_type: FaultType::Latency,
                            source,
                            tick: effective_tick,
                            position: pos,
                            paths_invalidated: 0,
                        });
                        fault_metrics.register_manual_event(
                            effective_tick,
                            FaultType::Latency,
                            source,
                            pos,
                            alive_count,
                        );
                        if is_replay && !causality_applied {
                            fault_log.truncate_after_tick(effective_tick);
                            tick_history.truncate_after_tick(effective_tick);
                            if let Some(ref mut s) = sim {
                                s.analysis.truncate_to_tick(effective_tick);
                            }
                            causality_applied = true;
                        }
                        fault_log.insert_sorted(ManualFaultEntry {
                            tick: effective_tick,
                            action: ManualFaultAction::InjectLatency {
                                agent_id: *agent_id,
                                duration: dur,
                            },
                            source,
                        });
                    }
                    // Also apply to runner
                    if let Some(ref mut sim) = sim
                        && *agent_id < sim.runner.agents.len()
                    {
                        sim.runner.agents[*agent_id].latency_remaining = dur;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_fault_command_variants_exist() {
        let _ = ManualFaultCommand::KillAgent(0);
        let _ = ManualFaultCommand::PlaceObstacle(IVec2::ZERO);
        let _ = ManualFaultCommand::InjectLatency { agent_id: 0, duration: 10 };
    }

    #[test]
    fn manual_fault_log_truncate() {
        let mut log = ManualFaultLog::default();
        log.entries.push(ManualFaultEntry {
            tick: 10,
            action: ManualFaultAction::PlaceObstacle(IVec2::ZERO),
            source: FaultSource::Manual,
        });
        log.entries.push(ManualFaultEntry {
            tick: 20,
            action: ManualFaultAction::PlaceObstacle(IVec2::ONE),
            source: FaultSource::Manual,
        });
        log.entries.push(ManualFaultEntry {
            tick: 30,
            action: ManualFaultAction::PlaceObstacle(IVec2::new(2, 2)),
            source: FaultSource::Manual,
        });

        log.truncate_after_tick(20);
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[1].tick, 20);
    }

    #[test]
    fn manual_fault_log_clear() {
        let mut log = ManualFaultLog::default();
        log.entries.push(ManualFaultEntry {
            tick: 10,
            action: ManualFaultAction::PlaceObstacle(IVec2::ZERO),
            source: FaultSource::Manual,
        });
        log.clear();
        assert!(log.entries.is_empty());
    }

    #[test]
    fn rewind_request_default_is_none() {
        let req = RewindRequest::default();
        assert!(req.pending.is_none());
    }
}
