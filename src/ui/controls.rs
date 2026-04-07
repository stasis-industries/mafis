#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::HashSet;

#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::cascade::CascadeState;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::dependency::{ActionDependencyGraph, AdgThrottle, BetweennessCriticality};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::fault_metrics::FaultMetrics;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::heatmap::HeatmapState;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::history::TickHistory;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::metrics::SimMetrics;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::analysis::scorecard::{ResilienceScorecard, ScorecardState};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::constants;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::agent::{AgentActionStats, AgentRegistry, LogicalAgent};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::grid::GridMap;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::phase::{ResilienceBaseline, SimulationPhase};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::seed::SeededRng;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::state::{LoadingPhase, LoadingProgress, SimState, SimulationConfig};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::task::LifelongConfig;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::core::topology::{ActiveTopology, ZoneMap, ZoneType};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::fault::heat::HeatState;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::render::environment::{
    EnvironmentMarker, ObstacleMarker, spawn_floor_and_lines, spawn_queue_arrows,
    spawn_zone_markers,
};
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::render::orbit_camera::OrbitCamera;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::solver::ActiveSolver;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::solver::heuristics::DistanceMapCache;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::solver::pibt::PibtSolver;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
use crate::solver::traits::MAPFSolver;

// ---------------------------------------------------------------------------
// SystemParam bundles (to stay under Bevy's 16-param limit)
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
#[derive(SystemParam)]
struct LoadingResources<'w> {
    grid: ResMut<'w, GridMap>,
    registry: ResMut<'w, AgentRegistry>,
    rng: ResMut<'w, SeededRng>,
    config: ResMut<'w, SimulationConfig>,
    ui_state: Res<'w, UiState>,
    topology: Res<'w, ActiveTopology>,
    zone_map: ResMut<'w, ZoneMap>,
    orbit: ResMut<'w, OrbitCamera>,
    progress: ResMut<'w, LoadingProgress>,
    fingerprint: ResMut<'w, ScenarioFingerprint>,
    lifelong: ResMut<'w, LifelongConfig>,
    phase: ResMut<'w, SimulationPhase>,
    baseline: ResMut<'w, ResilienceBaseline>,
    fault_scenario: ResMut<'w, crate::fault::scenario::FaultScenario>,
    fault_schedule: ResMut<'w, crate::fault::scenario::FaultSchedule>,
    fault_config: ResMut<'w, crate::fault::config::FaultConfig>,
    fault_list: Res<'w, crate::fault::scenario::FaultList>,
    baseline_store: ResMut<'w, crate::analysis::baseline::BaselineStore>,
    active_scheduler: Res<'w, crate::core::task::ActiveScheduler>,
}

/// Resources that must be reset between simulation runs for determinism.
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
#[derive(SystemParam)]
struct ResetResources<'w> {
    solver: ResMut<'w, ActiveSolver>,
    dist_cache: ResMut<'w, DistanceMapCache>,
    cascade: ResMut<'w, CascadeState>,
    fault_metrics: ResMut<'w, FaultMetrics>,
    tick_history: ResMut<'w, TickHistory>,
    heatmap: ResMut<'w, HeatmapState>,
    sim_metrics: ResMut<'w, SimMetrics>,
    scorecard: ResMut<'w, ResilienceScorecard>,
    scorecard_state: ResMut<'w, ScorecardState>,
    adg: ResMut<'w, ActionDependencyGraph>,
    adg_throttle: ResMut<'w, AdgThrottle>,
    criticality: ResMut<'w, BetweennessCriticality>,
    manual_fault_log: ResMut<'w, crate::fault::manual::ManualFaultLog>,
    baseline_diff: ResMut<'w, crate::analysis::baseline::BaselineDiff>,
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Holds parsed MovingAI .map + .scen data for imported benchmark scenarios.
pub struct ImportedScenario {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub obstacles: HashSet<IVec2>,
    pub agents: Vec<(IVec2, IVec2)>, // (start, goal) pairs
}

#[derive(Resource)]
pub struct UiState {
    pub num_agents: usize,
    pub seed: u64,
    pub obstacle_density: f32,
    pub grid_width: i32,
    pub grid_height: i32,
    pub solver_name: String,
    pub topology_name: String,
    pub imported_scenario: Option<ImportedScenario>,
    /// User override for RHCR horizon (None = auto).
    pub rhcr_horizon: Option<usize>,
    /// User override for RHCR replan interval (None = auto).
    pub rhcr_replan_interval: Option<usize>,
    /// Whether fault injection is enabled.
    pub fault_enabled: bool,
    /// Fault scenario type.
    pub fault_scenario_type: String,
    /// Burst: kill percent (0–100).
    pub burst_kill_percent: f32,
    /// Burst: at tick.
    pub burst_at_tick: u64,
    /// Wear: heat rate (low/medium/high).
    pub wear_heat_rate: String,
    /// Wear: overheat threshold.
    pub wear_threshold: f32,
    /// Zone: at tick.
    pub zone_at_tick: u64,
    /// Zone: latency duration.
    pub zone_latency_duration: u32,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            num_agents: 15,
            seed: 42,
            obstacle_density: 0.15,
            grid_width: 25,
            grid_height: 15,
            solver_name: "pibt".to_string(),
            topology_name: "warehouse_large".to_string(),
            imported_scenario: None,
            rhcr_horizon: None,
            rhcr_replan_interval: None,
            fault_enabled: false,
            fault_scenario_type: "burst_failure".to_string(),
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            wear_heat_rate: "medium".to_string(),
            wear_threshold: 80.0,
            zone_at_tick: 100,
            zone_latency_duration: 50,
        }
    }
}

// Everything below is observatory-only (render-dependent).
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
mod observatory_controls {
    use super::*;

    /// Custom map data from the map maker, ready for 3D preview in Idle state.
    #[derive(Resource)]
    pub struct PreviewMap {
        pub grid: GridMap,
        pub zones: ZoneMap,
        pub robot_starts: Vec<IVec2>,
        pub seed: u64,
        pub name: String,
    }

    /// Obstacle positions awaiting visual entity spawn (batched during Loading).
    #[derive(Resource)]
    pub struct LoadingObstacles(pub Vec<IVec2>);

    /// Agent (start, goal) pairs awaiting entity spawn (batched during Loading).
    #[derive(Resource)]
    pub struct LoadingAgents(pub Vec<(IVec2, IVec2)>);

    /// Fingerprint of the last spawned scenario for same-scenario relaunch optimization.
    #[derive(Resource, Default)]
    pub struct ScenarioFingerprint(pub Option<String>);

    // ---------------------------------------------------------------------------
    // Fingerprint
    // ---------------------------------------------------------------------------

    fn compute_fingerprint(ui: &UiState) -> String {
        if let Some(ref scen) = ui.imported_scenario {
            format!(
                "imported:{}:{}x{}:{}",
                scen.name,
                scen.width,
                scen.height,
                scen.obstacles.len()
            )
        } else {
            format!("topo:{}:{}x{}:{}", ui.topology_name, ui.grid_width, ui.grid_height, ui.seed)
        }
    }

    // ---------------------------------------------------------------------------
    // begin_loading — runs OnEnter(SimState::Loading)
    // ---------------------------------------------------------------------------

    fn begin_loading(
        mut commands: Commands,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
        mut res: LoadingResources,
        mut reset: ResetResources,
        queries: (
            Query<Entity, With<LogicalAgent>>,
            Query<Entity, With<ObstacleMarker>>,
            Query<Entity, With<EnvironmentMarker>>,
        ),
    ) {
        let (existing_agents, existing_obstacles, existing_env) = queries;

        let new_fp = compute_fingerprint(&res.ui_state);
        let same_scenario = res.fingerprint.0.as_ref() == Some(&new_fp);

        // Always reset these
        res.registry.clear();
        res.config.tick = 0;
        res.lifelong.reset();
        *res.phase = SimulationPhase::Running;
        res.baseline.reset();
        res.rng.reseed(res.ui_state.seed);

        // Recreate solver from ui_state to guarantee same type+config as baseline.
        // Just calling reset() would keep a stale solver type (e.g. RHCR from a
        // previous run) while the baseline creates a fresh one from solver_name.
        {
            let grid_area = (res.ui_state.grid_width * res.ui_state.grid_height) as usize;
            let num_agents = res.ui_state.num_agents;
            if let Some(fresh) = crate::solver::lifelong_solver_from_name(
                &res.ui_state.solver_name,
                grid_area,
                num_agents,
            ) {
                *reset.solver = ActiveSolver::new(fresh);
            } else {
                reset.solver.reset();
            }
        }
        reset.dist_cache.clear();
        reset.cascade.clear();
        reset.fault_metrics.clear();
        reset.tick_history.clear();
        reset.heatmap.clear();
        reset.sim_metrics.clear();
        reset.scorecard.clear();
        reset.scorecard_state.clear();
        reset.adg.clear();
        reset.adg_throttle.last_tick = 0;
        reset.criticality.clear();
        reset.manual_fault_log.clear();
        reset.baseline_diff.clear();

        // Always despawn agents
        for entity in &existing_agents {
            commands.entity(entity).despawn();
        }

        // Apply fault list → compile into FaultConfig + FaultSchedule
        {
            let total_ticks = res.config.duration;
            let (fc, fs) = res.fault_list.compile(total_ticks, res.ui_state.num_agents);
            *res.fault_config = fc;
            *res.fault_schedule = fs;
            res.fault_scenario.enabled = res.fault_list.is_active();

            // Set up incremental baseline computation — runs across multiple frames
            // during the Baseline loading phase to avoid blocking the main thread.
            {
                use crate::analysis::baseline::{BaselineConfig, start_headless};

                // Always provide grid_override so the baseline uses the exact same
                // grid as the LiveSim. Without this, registry topologies (e.g.
                // "warehouse_large") call ActiveTopology::from_name() which panics
                // on WASM and can produce subtly different grids on native.
                let grid_override = Some(generate_grid_and_zones(
                    &res.ui_state.topology_name,
                    &res.topology,
                    res.ui_state.seed,
                    res.ui_state.imported_scenario.as_ref(),
                ));

                let agent_positions =
                    res.ui_state.imported_scenario.as_ref().and_then(|s| {
                        if s.agents.is_empty() { None } else { Some(s.agents.clone()) }
                    });

                let baseline_config = BaselineConfig {
                    topology_name: res.ui_state.topology_name.clone(),
                    num_agents: res.ui_state.num_agents,
                    solver_name: res.ui_state.solver_name.clone(),
                    scheduler_name: res.active_scheduler.name().to_string(),
                    seed: res.ui_state.seed,
                    tick_count: total_ticks,
                    grid_override,
                    fault_enabled: res.fault_list.is_active(),
                    agent_positions,
                };
                res.baseline_store.computing = true;
                let computation = start_headless(&baseline_config);
                commands.insert_resource(computation);
            }
        }

        // ── Create LiveSim (runner + analysis engine) ──────────────────────
        // Agent positions are extracted BEFORE the runner takes ownership,
        // then reused for ECS entity spawning (LoadingAgents). This guarantees
        // ECS entities start at exactly the same positions as runner agents.
        let live_agent_pairs: Vec<(IVec2, IVec2)>;
        {
            use crate::analysis::baseline::place_agents;
            use crate::core::live_sim::LiveSim;
            use crate::core::runner::{SimAgent, SimulationRunner};

            let mut sim_rng = SeededRng::new(res.ui_state.seed);

            // Generate grid+zones for the runner (owns its own copy).
            // For custom maps, use the stored topology which preserves explicit
            // cell types from the map maker (pickup, delivery, corridor).
            // For standard topologies, use ActiveTopology::from_name to match
            // what run_headless does.
            let (runner_grid, runner_zones) = generate_grid_and_zones(
                &res.ui_state.topology_name,
                &res.topology,
                res.ui_state.seed,
                res.ui_state.imported_scenario.as_ref(),
            );

            // Clamp agent count to walkable capacity (matches experiment runner)
            let actual_agents = res.ui_state.num_agents.min(runner_grid.walkable_count());

            // Place agents using the same logic as baseline (identical RNG path).
            // When imported_scenario has no agents (e.g., simulateIn3D strips robots
            // from the topology JSON), fall through to place_agents() so the RNG
            // path matches the experiment runner.
            let sim_agents = if let Some(ref scenario) = res.ui_state.imported_scenario {
                if !scenario.agents.is_empty() {
                    scenario.agents.iter().map(|&(s, _)| SimAgent::new(s)).collect()
                } else {
                    place_agents(actual_agents, &runner_grid, &runner_zones, &mut sim_rng)
                }
            } else {
                place_agents(actual_agents, &runner_grid, &runner_zones, &mut sim_rng)
            };

            // Extract positions for ECS entity spawning BEFORE runner takes ownership
            live_agent_pairs = sim_agents.iter().map(|a| (a.pos, a.goal)).collect();

            // Create solver — use actual_agents (clamped) so RHCR auto-config
            // matches what the experiment runner and baseline engine compute.
            let runner_solver = crate::solver::lifelong_solver_from_name(
                &res.ui_state.solver_name,
                (runner_grid.width * runner_grid.height) as usize,
                actual_agents,
            )
            .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

            let runner = SimulationRunner::new(
                runner_grid,
                runner_zones,
                sim_agents,
                runner_solver,
                sim_rng,
                res.fault_config.clone(),
                res.fault_schedule.clone(),
            );

            commands.insert_resource(LiveSim::new(runner, res.config.duration as usize));
        }

        if same_scenario {
            // ── Same scenario relaunch ──────────────────────────────────────
            // Despawn obstacle visuals and environment markers (zone tiles, queue arrows).
            // Faults corrupt the grid, so we must rebuild from topology.
            // Queue arrows + zone markers must be respawned with the fresh zone_map.
            for entity in &existing_obstacles {
                commands.entity(entity).despawn();
            }
            for entity in &existing_env {
                commands.entity(entity).despawn();
            }

            // CRITICAL: Rebuild grid from topology for determinism.
            // Faults modify the grid at runtime (grid.set_obstacle), so reusing
            // the old grid would place agents on a corrupted map.
            // Use ActiveTopology::from_name (not res.topology) to match runner/baseline.
            let (new_grid, new_zones) = generate_grid_and_zones(
                &res.ui_state.topology_name,
                &res.topology,
                res.ui_state.seed,
                res.ui_state.imported_scenario.as_ref(),
            );
            *res.grid = new_grid;
            *res.zone_map = new_zones;

            // Respawn floor + zone markers + queue arrows (environment was despawned above)
            spawn_floor_and_lines(&mut commands, &mut meshes, &mut materials, &res.grid);
            spawn_zone_markers(&mut commands, &mut meshes, &mut materials, &res.zone_map);
            spawn_queue_arrows(&mut commands, &mut meshes, &mut materials, &res.zone_map);

            // Queue obstacle visuals for batched spawn
            let obstacles: Vec<IVec2> = res.grid.obstacles().iter().copied().collect();
            let obs_total = obstacles.len();
            commands.insert_resource(LoadingObstacles(obstacles));

            commands.insert_resource(LoadingAgents(live_agent_pairs.clone()));

            *res.progress =
                LoadingProgress { phase: LoadingPhase::Obstacles, current: 0, total: obs_total };
        } else {
            // ── New scenario — full rebuild ──────────────────────────────────

            // Despawn obstacles + environment
            for entity in &existing_obstacles {
                commands.entity(entity).despawn();
            }
            for entity in &existing_env {
                commands.entity(entity).despawn();
            }

            // Build grid data via topology.
            // Use ActiveTopology::from_name (not res.topology) to match runner/baseline.
            let (new_grid, new_zones) = generate_grid_and_zones(
                &res.ui_state.topology_name,
                &res.topology,
                res.ui_state.seed,
                res.ui_state.imported_scenario.as_ref(),
            );
            *res.grid = new_grid;
            *res.zone_map = new_zones;

            // Spawn floor + grid lines (instant, few entities)
            spawn_floor_and_lines(&mut commands, &mut meshes, &mut materials, &res.grid);

            // Spawn zone markers (delivery/pickup colored floor tiles)
            spawn_zone_markers(&mut commands, &mut meshes, &mut materials, &res.zone_map);
            spawn_queue_arrows(&mut commands, &mut meshes, &mut materials, &res.zone_map);

            // Update orbit camera
            let center_x = (res.grid.width as f32 - 1.0) * 0.5;
            let center_z = (res.grid.height as f32 - 1.0) * 0.5;
            res.orbit.focus = Vec3::new(center_x, 0.0, center_z);
            let extent = (res.grid.width as f32).max(res.grid.height as f32);
            res.orbit.min_distance = extent * 0.3;
            res.orbit.max_distance = extent * 3.0;

            // Collect obstacle positions for batched visual spawn
            let obstacles: Vec<IVec2> = res.grid.obstacles().iter().copied().collect();
            let obs_total = obstacles.len();
            commands.insert_resource(LoadingObstacles(obstacles));

            // Use runner's agent positions for ECS entity spawning (parity guarantee)
            commands.insert_resource(LoadingAgents(live_agent_pairs));

            // Update fingerprint
            res.fingerprint.0 = Some(new_fp);

            *res.progress =
                LoadingProgress { phase: LoadingPhase::Obstacles, current: 0, total: obs_total };
        }
    }

    // ---------------------------------------------------------------------------
    // loading_tick — runs in Update while in SimState::Loading
    // ---------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn loading_tick(
        mut commands: Commands,
        mut progress: ResMut<LoadingProgress>,
        mut loading_obstacles: Option<ResMut<LoadingObstacles>>,
        mut loading_agents: Option<ResMut<LoadingAgents>>,
        mut baseline_comp: Option<ResMut<crate::analysis::baseline::BaselineComputation>>,
        mut baseline_store: ResMut<crate::analysis::baseline::BaselineStore>,
        mut registry: ResMut<AgentRegistry>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
        grid: Res<GridMap>,
        mut agents_query: Query<&mut LogicalAgent>,
        mut next_state: ResMut<NextState<SimState>>,
        mut obs_handles: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
        resume_target: Res<crate::core::state::ResumeTarget>,
        mut fixed_time: ResMut<Time<Fixed>>,
    ) {
        match progress.phase {
            LoadingPhase::Obstacles => {
                // Initialize shared handles on first frame
                let (mesh_h, mat_h) = obs_handles.get_or_insert_with(|| {
                    let mesh = meshes.add(Cuboid::new(0.9, 0.6, 0.9));
                    let mat = materials.add(StandardMaterial {
                        base_color: Color::srgb(0.25, 0.25, 0.3),
                        perceptual_roughness: 0.7,
                        ..default()
                    });
                    (mesh, mat)
                });

                if let Some(ref mut obs) = loading_obstacles {
                    let count = obs.0.len().min(constants::LOADING_OBSTACLE_BATCH);
                    let batch = obs.0.drain(..count).collect::<Vec<_>>();

                    for pos in &batch {
                        let world = crate::render::environment::grid_to_world(*pos);
                        commands.spawn((
                            Mesh3d(mesh_h.clone()),
                            MeshMaterial3d(mat_h.clone()),
                            Transform::from_xyz(world.x, 0.3, world.z),
                            ObstacleMarker,
                        ));
                    }
                    progress.current += batch.len();

                    if obs.0.is_empty() {
                        commands.remove_resource::<LoadingObstacles>();
                        if let Some(ref agents) = loading_agents {
                            progress.total = agents.0.len();
                        }
                        progress.current = 0;
                        progress.phase = LoadingPhase::Agents;
                        *obs_handles = None;
                    }
                } else {
                    progress.phase = LoadingPhase::Agents;
                    progress.current = 0;
                    if let Some(ref agents) = loading_agents {
                        progress.total = agents.0.len();
                    }
                }
            }

            LoadingPhase::Agents => {
                if let Some(ref mut agents) = loading_agents {
                    let count = agents.0.len().min(constants::LOADING_AGENT_BATCH);
                    let batch = agents.0.drain(..count).collect::<Vec<_>>();

                    for (start, goal) in &batch {
                        let entity = commands
                            .spawn((
                                LogicalAgent::new(*start, *goal),
                                HeatState::default(),
                                AgentActionStats::default(),
                            ))
                            .id();
                        let index = registry.register(entity);
                        commands.entity(entity).insert(index);
                    }
                    progress.current += batch.len();

                    if agents.0.is_empty() {
                        commands.remove_resource::<LoadingAgents>();
                        // Move to baseline computation phase
                        if let Some(ref comp) = baseline_comp {
                            progress.total = comp.total_ticks as usize;
                        }
                        progress.current = 0;
                        progress.phase = LoadingPhase::Baseline;
                    }
                } else {
                    if let Some(ref comp) = baseline_comp {
                        progress.total = comp.total_ticks as usize;
                    }
                    progress.current = 0;
                    progress.phase = LoadingPhase::Baseline;
                }
            }

            LoadingPhase::Baseline => {
                if let Some(ref mut comp) = baseline_comp {
                    let done = comp.tick_batch(constants::BASELINE_TICKS_PER_FRAME);
                    progress.current = comp.ticks_done as usize;

                    if done {
                        let record = comp.take_record();

                        #[cfg(target_arch = "wasm32")]
                        {
                            let msg = format!(
                                "[BASELINE] ticks={}, agents={} → total_tasks={}, avg_tp={:.2}",
                                record.tick_count,
                                record.num_agents,
                                record.total_tasks,
                                record.avg_throughput,
                            );
                            web_sys::console::log_1(&msg.into());
                        }

                        baseline_store.record = Some(record);
                        baseline_store.computing = false;
                        commands
                            .remove_resource::<crate::analysis::baseline::BaselineComputation>();

                        progress.phase = LoadingPhase::Solving;
                        progress.current = 0;
                        progress.total = 1;
                    }
                } else {
                    baseline_store.computing = false;
                    progress.phase = LoadingPhase::Solving;
                    progress.current = 0;
                    progress.total = 1;
                }
            }

            LoadingPhase::Solving => {
                // Invoke initial solver (blocking)
                let agent_data: Vec<(IVec2, IVec2)> =
                    agents_query.iter().map(|a| (a.current_pos, a.goal_pos)).collect();

                if !agent_data.is_empty() {
                    let needs_solve = agent_data.iter().any(|(s, g)| s != g);
                    if needs_solve {
                        let paths: Vec<Vec<crate::core::action::Action>> =
                            PibtSolver::default().solve(&grid, &agent_data).unwrap_or_default();
                        for (mut agent, path) in agents_query.iter_mut().zip(paths.into_iter()) {
                            agent.planned_path = path.into();
                            agent.path_length = agent.planned_path.len();
                        }
                    }
                }
                progress.current = 1;
                progress.phase = LoadingPhase::Done;
            }

            LoadingPhase::Done => {
                if resume_target.target_tick.is_some() {
                    *fixed_time = Time::<Fixed>::from_hz(10000.0);
                }
                next_state.set(SimState::Running);
            }

            LoadingPhase::Setup => {
                // Unreachable
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Grid/zone generation helper
    // ---------------------------------------------------------------------------

    /// Generate a (GridMap, ZoneMap) pair from the current topology/scenario settings.
    ///
    /// For "custom" maps or registry-loaded maps: uses the active topology resource.
    /// For imported scenarios: builds grid from scenario obstacles + classifies zones.
    /// Fallback: loads from registry via `ActiveTopology::from_name`.
    fn generate_grid_and_zones(
        topology_name: &str,
        topology: &ActiveTopology,
        seed: u64,
        imported_scenario: Option<&ImportedScenario>,
    ) -> (GridMap, ZoneMap) {
        if topology.name() == "custom" || topology.name() == topology_name {
            // Registry-loaded or custom map with explicit zones — use directly.
            let output = topology.topology().generate(seed);
            (output.grid, output.zones)
        } else if let Some(scenario) = imported_scenario {
            // Imported .map file without explicit zones — classify heuristically.
            let grid = GridMap::with_obstacles(
                scenario.width,
                scenario.height,
                scenario.obstacles.clone(),
            );
            let zones = classify_imported_zones(&grid);
            (grid, zones)
        } else {
            // Topology name doesn't match active — regenerate from active anyway.
            // On WASM, from_name is unavailable; the topology should already be set
            // via the bridge before begin_loading is called.
            let output = topology.topology().generate(seed);
            (output.grid, output.zones)
        }
    }

    // ---------------------------------------------------------------------------
    // Imported map zone classification
    // ---------------------------------------------------------------------------

    /// Detect warehouse structure and assign zones for imported .map files.
    ///
    /// Warehouse maps have storage blocks (runs of ≥5 consecutive obstacles in a row).
    /// - Cells in left/right open margins (outside storage columns) → Delivery
    /// - Cells adjacent (4-dir) to any obstacle → Pickup (picking aisles)
    /// - Remaining walkable cells in storage area → Corridor
    ///
    /// Non-warehouse maps (random obstacles) → all cells Open.
    fn classify_imported_zones(grid: &GridMap) -> ZoneMap {
        let w = grid.width;
        let h = grid.height;

        // Detect warehouse structure: find columns that contain obstacle runs ≥ 5.
        let mut has_storage_block = vec![false; w as usize];
        for y in 0..h {
            let mut run = 0i32;
            let mut run_start = 0i32;
            for x in 0..w {
                if !grid.is_walkable(IVec2::new(x, y)) {
                    if run == 0 {
                        run_start = x;
                    }
                    run += 1;
                } else {
                    if run >= 5 {
                        for col in run_start..(run_start + run) {
                            has_storage_block[col as usize] = true;
                        }
                    }
                    run = 0;
                }
            }
            if run >= 5 {
                for col in run_start..(run_start + run) {
                    has_storage_block[col as usize] = true;
                }
            }
        }

        // Find leftmost and rightmost storage columns (skip border column 0 and w-1)
        let storage_left = has_storage_block.iter().position(|&b| b);
        let storage_right = has_storage_block.iter().rposition(|&b| b);

        let is_warehouse = storage_left.is_some();

        let mut zm = ZoneMap::default();

        if !is_warehouse {
            // Non-warehouse: all walkable cells = Open
            for x in 0..w {
                for y in 0..h {
                    let pos = IVec2::new(x, y);
                    if grid.is_walkable(pos) {
                        zm.zone_type.insert(pos, ZoneType::Open);
                        zm.pickup_cells.push(pos);
                        zm.delivery_cells.push(pos);
                        zm.corridor_cells.push(pos);
                    }
                }
            }
            return zm;
        }

        let sl = storage_left.unwrap() as i32;
        let sr = storage_right.unwrap() as i32;

        for x in 0..w {
            for y in 0..h {
                let pos = IVec2::new(x, y);
                if !grid.is_walkable(pos) {
                    continue;
                }

                if x < sl || x > sr {
                    // Outside storage columns → delivery zone
                    zm.zone_type.insert(pos, ZoneType::Delivery);
                    zm.delivery_cells.push(pos);
                } else {
                    // Inside storage area: check adjacency to obstacles
                    let adjacent_to_obstacle = [
                        IVec2::new(x - 1, y),
                        IVec2::new(x + 1, y),
                        IVec2::new(x, y - 1),
                        IVec2::new(x, y + 1),
                    ]
                    .iter()
                    .any(|&n| n.x >= 0 && n.x < w && n.y >= 0 && n.y < h && !grid.is_walkable(n));

                    if adjacent_to_obstacle {
                        zm.zone_type.insert(pos, ZoneType::Pickup);
                        zm.pickup_cells.push(pos);
                    } else {
                        zm.zone_type.insert(pos, ZoneType::Corridor);
                        zm.corridor_cells.push(pos);
                    }
                }
            }
        }

        zm
    }

    // Placement helpers moved to core::placement (shared with headless baseline)

    // ---------------------------------------------------------------------------
    // Reset
    // ---------------------------------------------------------------------------

    fn despawn_on_reset(
        mut commands: Commands,
        mut config: ResMut<SimulationConfig>,
        mut registry: ResMut<AgentRegistry>,
        mut phase: ResMut<SimulationPhase>,
        mut baseline: ResMut<ResilienceBaseline>,
        mut manual_fault_log: ResMut<crate::fault::manual::ManualFaultLog>,
        existing_agents: Query<Entity, With<LogicalAgent>>,
        existing_obstacles: Query<Entity, With<ObstacleMarker>>,
        grid: Res<GridMap>,
        zone_map: Res<ZoneMap>,
        ui_state: Res<UiState>,
    ) {
        for entity in &existing_agents {
            commands.entity(entity).despawn();
        }
        // Despawn obstacle visuals so fault-injected walls don't linger during
        // the Idle state. begin_loading rebuilds them.
        for entity in &existing_obstacles {
            commands.entity(entity).despawn();
        }
        config.tick = 0;
        registry.clear();
        *phase = SimulationPhase::Running;
        baseline.reset();
        manual_fault_log.clear();

        // Clean up any leftover loading resources
        commands.remove_resource::<LoadingObstacles>();
        commands.remove_resource::<LoadingAgents>();
        commands.remove_resource::<crate::core::live_sim::LiveSim>();

        // Re-insert PreviewMap so obstacles respawn in the 3D preview
        commands.insert_resource(PreviewMap {
            grid: grid.clone(),
            zones: zone_map.clone(),
            robot_starts: Vec::new(),
            seed: ui_state.seed,
            name: ui_state.topology_name.clone(),
        });
    }

    // ---------------------------------------------------------------------------
    // Preview spawning — renders custom map in 3D while still in Idle
    // ---------------------------------------------------------------------------

    /// Marker for preview robot entities (static, non-simulating).
    #[derive(Component)]
    pub struct PreviewRobot;

    fn spawn_preview(
        mut commands: Commands,
        preview: Option<Res<PreviewMap>>,
        mut meshes: ResMut<Assets<Mesh>>,
        mut materials: ResMut<Assets<StandardMaterial>>,
        mut ui_state: ResMut<UiState>,
        mut orbit: ResMut<OrbitCamera>,
        mut grid: ResMut<GridMap>,
        mut zone_map: ResMut<ZoneMap>,
        existing_obstacles: Query<Entity, With<ObstacleMarker>>,
        existing_env: Query<Entity, With<EnvironmentMarker>>,
        existing_preview_robots: Query<Entity, With<PreviewRobot>>,
    ) {
        let preview = match preview {
            Some(p) => p,
            None => return,
        };

        // Despawn existing scene
        for entity in &existing_obstacles {
            commands.entity(entity).despawn();
        }
        for entity in &existing_env {
            commands.entity(entity).despawn();
        }
        for entity in &existing_preview_robots {
            commands.entity(entity).despawn();
        }

        // Apply grid + zones
        *grid = GridMap::with_obstacles(
            preview.grid.width,
            preview.grid.height,
            preview.grid.obstacles().clone(),
        );
        *zone_map = preview.zones.clone();

        // Update UI state
        ui_state.grid_width = preview.grid.width;
        ui_state.grid_height = preview.grid.height;
        ui_state.seed = preview.seed;
        // Only override num_agents when robots are explicitly placed in the preview.
        // When robots are empty (e.g. simulateIn3D strips them), keep the value
        // set by set_num_agents so begin_loading uses the experiment's agent count.
        if !preview.robot_starts.is_empty() {
            ui_state.num_agents = preview.robot_starts.len();
        }
        ui_state.obstacle_density = preview.grid.obstacle_count() as f32
            / (preview.grid.width * preview.grid.height) as f32;
        ui_state.topology_name = "custom".to_string();
        // Keep imported_scenario — begin_loading needs it for robot start positions

        // Spawn floor + grid lines
        spawn_floor_and_lines(&mut commands, &mut meshes, &mut materials, &grid);

        // Spawn zone markers
        spawn_zone_markers(&mut commands, &mut meshes, &mut materials, &zone_map);
        spawn_queue_arrows(&mut commands, &mut meshes, &mut materials, &zone_map);

        // Spawn obstacles
        crate::render::environment::spawn_obstacles(
            &mut commands,
            &mut meshes,
            &mut materials,
            &grid,
        );

        // Spawn preview robots (static cylinders at start positions)
        let robot_mesh = meshes.add(Cylinder::new(0.35, 0.14));
        let robot_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.57, 0.60),
            perceptual_roughness: 0.4,
            metallic: 0.2,
            ..default()
        });
        for &pos in &preview.robot_starts {
            let world = crate::render::environment::grid_to_world(pos);
            commands.spawn((
                Mesh3d(robot_mesh.clone()),
                MeshMaterial3d(robot_mat.clone()),
                Transform::from_xyz(world.x, 0.07, world.z),
                PreviewRobot,
            ));
        }

        // Update orbit camera
        let center_x = (grid.width as f32 - 1.0) * 0.5;
        let center_z = (grid.height as f32 - 1.0) * 0.5;
        orbit.focus = Vec3::new(center_x, 0.0, center_z);
        let extent = (grid.width as f32).max(grid.height as f32);
        orbit.min_distance = extent * 0.3;
        orbit.max_distance = extent * 3.0;

        // Consume the resource
        commands.remove_resource::<PreviewMap>();
    }

    /// Despawn preview robots when entering Loading (real agents will spawn).
    fn despawn_preview_robots(mut commands: Commands, robots: Query<Entity, With<PreviewRobot>>) {
        for entity in &robots {
            commands.entity(entity).despawn();
        }
    }

    // ---------------------------------------------------------------------------
    // Plugin
    // ---------------------------------------------------------------------------

    pub struct ControlsPlugin;

    impl Plugin for ControlsPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<UiState>()
                .init_resource::<ScenarioFingerprint>()
                .add_systems(OnEnter(SimState::Loading), (begin_loading, despawn_preview_robots))
                .add_systems(
                    Update,
                    (
                        loading_tick.run_if(in_state(SimState::Loading)),
                        spawn_preview.run_if(in_state(SimState::Idle)),
                    ),
                )
                .add_systems(OnEnter(SimState::Idle), despawn_on_reset);
        }
    }
} // mod observatory_controls

#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
pub use observatory_controls::*;
