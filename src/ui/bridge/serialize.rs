use bevy::prelude::*;
use bevy::ecs::system::SystemParam;
use serde::Serialize;

use crate::analysis::baseline::BaselineStore;
use crate::analysis::cascade::CascadeState;
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::heatmap::{HeatmapMode, HeatmapState};
use crate::analysis::history::TickHistory;
use crate::analysis::metrics::SimMetrics;
use crate::analysis::scorecard::ResilienceScorecard;
use crate::analysis::{AnalysisConfig, MetricsConfig, TimeSeriesAccessor};
use crate::constants;
use crate::core::agent::{AgentActionStats, AgentIndex, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::phase::{ResilienceBaseline, SimulationPhase};
use crate::core::state::{LoadingPhase, LoadingProgress, SimState, SimulationConfig};
use crate::core::task::{ActiveScheduler, LifelongConfig, TaskLeg};
use crate::core::topology::ActiveTopology;
use crate::export::config::ExportConfig;
use crate::fault::breakdown::{Dead, LatencyFault};
use crate::fault::config::FaultConfig;
use crate::fault::heat::HeatState;
use crate::fault::scenario::{FaultScenario, FaultSchedule, ScheduledAction};
use crate::render::animator::RobotOpacity;
use crate::render::graphics::GraphicsConfig;
use crate::render::orbit_camera::{CameraMode, OrbitCamera};
use crate::render::picking::ClickSelection;
use crate::solver::ActiveSolver;

use super::FpsTracker;
use crate::ui::controls::UiState;

// ---------------------------------------------------------------------------
// Serializable output for JS
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct BridgeOutput {
    pub version: &'static str,
    pub state: String,
    pub tick: u64,
    pub duration: u64,
    pub tick_hz: f64,
    pub num_agents: usize,
    pub seed: u64,
    pub obstacle_density: f32,
    pub grid_width: i32,
    pub grid_height: i32,
    pub total_agents: usize,
    pub alive_agents: usize,
    pub dead_agents: usize,
    pub fault_config: FaultConfigSnapshot,
    pub analysis_config: AnalysisConfigSnapshot,
    pub metrics_config: MetricsConfigSnapshot,
    pub export_config: ExportConfigSnapshot,
    pub metrics: Option<MetricsSnapshot>,
    pub solver: String,
    pub solver_info: SolverInfoSnapshot,
    pub agents: Vec<AgentSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_summary: Option<AgentSummary>,
    pub fps: f32,
    pub max_agents: usize,
    pub map_capacity: usize,
    pub max_grid_dim: i32,
    pub scenario_loaded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loading: Option<LoadingSnapshot>,
    pub robot_opacity: f32,
    pub camera_mode: String,
    pub graphics: GraphicsSnapshot,
    pub topology: String,
    pub storage_rows: usize,
    pub task_leg_counts: TaskLegCounts,
    pub lifelong: LifelongSnapshot,
    pub phase: PhaseSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<ReplaySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scorecard: Option<ScorecardSnapshot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fault_events: Vec<FaultEventSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click_selection: Option<ClickSelectionSnapshot>,
    pub fault_scenario: FaultScenarioSnapshot,
    pub baseline_diff: BaselineDiffSnapshot,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fault_schedule_markers: Vec<ScheduleMarkerSnapshot>,
}

#[derive(Serialize)]
pub(super) struct ClickSelectionSnapshot {
    agent_index: Option<usize>,
    cell: Option<[i32; 2]>,
    screen_x: f32,
    screen_y: f32,
}

#[derive(Serialize)]
pub(super) struct ReplaySnapshot {
    cursor_tick: u64,
    cursor_index: usize,
    total_recorded: usize,
    max_tick: u64,
}

#[derive(Serialize)]
pub(super) struct FaultEventSnapshot {
    id: usize,
    tick: u64,
    fault_type: &'static str,
    source: &'static str,
    position: [i32; 2],
    agents_affected: u32,
    cascade_depth: u32,
    throughput_before: f32,
    throughput_min: f32,
    throughput_delta: f32,
    recovered: bool,
    recovery_tick: Option<u64>,
}

#[derive(Serialize)]
pub(super) struct ScorecardSnapshot {
    fault_tolerance: f32,
    nrr: Option<f32>,
    fleet_utilization: f32,
    critical_time: f32,
    has_faults: bool,
}

#[derive(Serialize)]
pub(super) struct FaultScenarioSnapshot {
    enabled: bool,
    scenario_type: String,
    label: String,
    needs_baseline: bool,
    // Per-scenario params
    burst_kill_percent: f32,
    burst_at_tick: u64,
    wear_heat_rate: String,
    wear_threshold: f32,
    zone_at_tick: u64,
    zone_latency_duration: u32,
    perm_zone_at_tick: u64,
    perm_zone_block_percent: f32,
}

#[derive(Serialize)]
pub(super) struct BaselineDiffSnapshot {
    has_baseline: bool,
    computing: bool,
    baseline_throughput: f64,
    throughput_delta: f64,
    tasks_delta: i64,
    baseline_total_tasks: u64,
    baseline_tasks_at_tick: u64,
    baseline_idle_at_tick: usize,
    baseline_wait_ratio_at_tick: f32,
    baseline_avg_throughput: f64,
    // Differential metrics (from BaselineDiff resource)
    gap: i64,
    deficit_integral: i64,
    surplus_integral: i64,
    net_integral: i64,
    impacted_area: f64,
    rate_delta: f64,
    recovery_tick: Option<u64>,
}

#[derive(Serialize)]
pub(super) struct ScheduleMarkerSnapshot {
    tick: u64,
    marker_type: String,
    label: String,
    fired: bool,
}

#[derive(Serialize)]
pub(super) struct LifelongSnapshot {
    enabled: bool,
    tasks_completed: u64,
    throughput: f64,
    scheduler: String,
}

#[derive(Serialize)]
pub(super) struct TaskLegCounts {
    free: usize,
    travel_empty: usize,
    loading: usize,
    travel_to_queue: usize,
    queuing: usize,
    travel_loaded: usize,
    unloading: usize,
    charging: usize,
}

#[derive(Serialize)]
pub(super) struct LoadingSnapshot {
    phase: String,
    current: usize,
    total: usize,
    percent: f32,
}

#[derive(Serialize)]
pub(super) struct AgentSummary {
    total: usize,
    alive: usize,
    dead: usize,
    avg_heat: f32,
    max_heat: f32,
    avg_idle_ratio: f32,
    heat_histogram: [u32; 10],
}

#[derive(Serialize)]
pub(super) struct SolverInfoSnapshot {
    optimality: String,
    complexity: String,
    scalability: String,
    description: String,
    source: String,
    recommended_max_agents: Option<usize>,
}

#[derive(Serialize)]
pub(super) struct AgentSnapshot {
    id: usize,
    pos: [i32; 2],
    goal: [i32; 2],
    heat: f32,
    heat_normalized: f32,
    is_dead: bool,
    has_latency: bool,
    path_length: usize,
    distance_to_goal: i32,
    task_leg: String,
    idle_ratio: f32,
}

#[derive(Serialize)]
pub(super) struct FaultConfigSnapshot {
    enabled: bool,
    weibull_enabled: bool,
    weibull_beta: f32,
    weibull_eta: f32,
    intermittent_enabled: bool,
    intermittent_mtbf_ticks: u64,
    intermittent_recovery_ticks: u32,
}

#[derive(Serialize)]
pub(super) struct AnalysisConfigSnapshot {
    heatmap_visible: bool,
    heatmap_mode: String,
    heatmap_density_radius: i32,
    path_visible: bool,
}

#[derive(Serialize)]
pub(super) struct MetricsConfigSnapshot {
    aet: bool,
    makespan: bool,
    mttr: bool,
    fault_count: bool,
    cascade_depth: bool,
    cascade_cost: bool,
    fault_mttr: bool,
    recovery_rate: bool,
    cascade_spread: bool,
    throughput: bool,
    idle_ratio: bool,
}

#[derive(Serialize)]
pub(super) struct ExportConfigSnapshot {
    auto_on_finished: bool,
    auto_on_fault: bool,
    periodic_enabled: bool,
    periodic_interval: u64,
    export_json: bool,
    export_csv: bool,
}

#[derive(Serialize)]
pub(super) struct PhaseSnapshot {
    phase: String,
    baseline_throughput: f64,
    baseline_idle_ratio: f32,
}

#[derive(Serialize)]
pub(super) struct GraphicsSnapshot {
    shadows: bool,
    msaa: bool,
    colorblind: bool,
}

#[derive(Serialize)]
pub(super) struct MetricsSnapshot {
    aet: f32,
    makespan: u64,
    mttr: f32,
    fault_count: u32,
    max_cascade_depth: u32,
    total_cascade_cost: u32,
    fault_mttr: f32,
    fault_mtbf: Option<f32>,
    recovery_rate: f32,
    avg_cascade_spread: f32,
    propagation_rate: f32,
    throughput: f32,
    idle_ratio: f32,
    /// Number of agents with TaskLeg::Free (no task assigned).
    /// Used by charts for apples-to-apples baseline comparison.
    task_idle_count: usize,
    survival_rate: f32,
}

// ---------------------------------------------------------------------------
// Bevy system params
// ---------------------------------------------------------------------------

#[derive(SystemParam)]
pub(super) struct AnalysisResources<'w> {
    sim_metrics: Res<'w, SimMetrics>,
    cascade: Res<'w, CascadeState>,
    heatmap_state: Res<'w, HeatmapState>,
    fault_metrics: Res<'w, FaultMetrics>,
    tick_history: Res<'w, TickHistory>,
    scorecard: Res<'w, ResilienceScorecard>,
}

#[derive(SystemParam)]
pub(super) struct PhaseResources<'w> {
    phase: Res<'w, SimulationPhase>,
    baseline: Res<'w, ResilienceBaseline>,
    lifelong: Res<'w, LifelongConfig>,
    active_scheduler: Res<'w, ActiveScheduler>,
    active_topology: Res<'w, ActiveTopology>,
    orbit: Res<'w, OrbitCamera>,
    graphics: Res<'w, GraphicsConfig>,
    metrics_config: Res<'w, MetricsConfig>,
    fault_scenario: Res<'w, FaultScenario>,
    baseline_store: Res<'w, BaselineStore>,
    fault_schedule: Res<'w, FaultSchedule>,
    resume_target: Res<'w, crate::core::state::ResumeTarget>,
    baseline_diff: Res<'w, crate::analysis::baseline::BaselineDiff>,
    grid: Res<'w, GridMap>,
}

// ---------------------------------------------------------------------------
// Adaptive sync interval
// ---------------------------------------------------------------------------

fn bridge_sync_interval(agent_count: usize) -> f32 {
    if agent_count > 400 {
        constants::BRIDGE_SYNC_INTERVAL_XLARGE
    } else if agent_count > 200 {
        constants::BRIDGE_SYNC_INTERVAL_SLOW
    } else if agent_count > constants::AGGREGATE_THRESHOLD {
        constants::BRIDGE_SYNC_INTERVAL_MED
    } else {
        constants::BRIDGE_SYNC_INTERVAL_FAST
    }
}

// ---------------------------------------------------------------------------
// sync_state_to_js system
// ---------------------------------------------------------------------------

pub(super) fn sync_state_to_js(
    sim_state: Res<State<SimState>>,
    config: Res<SimulationConfig>,
    ui_state: Res<UiState>,
    fault_config: Res<FaultConfig>,
    analysis_config: Res<AnalysisConfig>,
    export_config: Res<ExportConfig>,
    analysis: AnalysisResources,
    solver: Res<ActiveSolver>,
    time: Res<Time>,
    mut fps_tracker: ResMut<FpsTracker>,
    mut sync_timer: Local<f32>,
    loading_progress: Res<LoadingProgress>,
    phase_res: PhaseResources,
    robot_opacity: Res<RobotOpacity>,
    mut click_selection: ResMut<ClickSelection>,
    agents_query: Query<(
        &LogicalAgent,
        &AgentIndex,
        Option<&HeatState>,
        Option<&AgentActionStats>,
        Has<Dead>,
        Has<LatencyFault>,
    )>,
) {
    // Suppress JS sync during fast-forward to maximize tick throughput
    if phase_res.resume_target.target_tick.is_some() {
        return;
    }

    let delta = time.delta_secs();
    if delta > 0.0 {
        let raw = 1.0 / delta;
        fps_tracker.smoothed = fps_tracker.smoothed * 0.95 + raw * 0.05;
    }

    let current = *sim_state.get();

    // Force sync every frame during Loading; use adaptive interval otherwise
    if current != SimState::Loading {
        *sync_timer += delta;
        let interval = bridge_sync_interval(ui_state.num_agents);
        if *sync_timer < interval {
            return;
        }
        *sync_timer = 0.0;
    }

    let state_str = match current {
        SimState::Idle => "idle",
        SimState::Loading => "loading",
        SimState::Running => "running",
        SimState::Paused => "paused",
        SimState::Replay => "replay",
        SimState::Finished => "finished",
    };

    let mut metrics = if phase_res.metrics_config.any() {
        let latest_survival =
            analysis.fault_metrics.survival_series.back().map(|(_, rate)| *rate).unwrap_or(1.0);
        Some(MetricsSnapshot {
            aet: analysis.sim_metrics.aet,
            makespan: analysis.sim_metrics.makespan,
            mttr: analysis.sim_metrics.mttr,
            fault_count: analysis.cascade.fault_count,
            max_cascade_depth: analysis.cascade.max_depth,
            total_cascade_cost: analysis.cascade.fault_count,
            fault_mttr: analysis.fault_metrics.mttr,
            fault_mtbf: analysis.fault_metrics.mtbf,
            recovery_rate: analysis.fault_metrics.recovery_rate,
            avg_cascade_spread: analysis.fault_metrics.avg_cascade_spread,
            propagation_rate: analysis.fault_metrics.propagation_rate,
            throughput: phase_res.lifelong.throughput(config.tick) as f32,
            idle_ratio: analysis.fault_metrics.idle_ratio,
            task_idle_count: 0, // filled below after agent loop
            survival_rate: latest_survival,
        })
    } else {
        None
    };

    // Use ui_state.num_agents to decide aggregate mode without iterating query twice
    let use_aggregate = ui_state.num_agents > constants::AGGREGATE_THRESHOLD;

    let mut agents_data: Vec<AgentSnapshot> = Vec::new();
    let mut total = 0usize;
    let mut alive = 0usize;
    let mut dead = 0usize;
    let mut heat_sum = 0.0f32;
    let mut max_heat = 0.0f32;
    let mut idle_sum = 0.0f32;
    let mut heat_histogram = [0u32; 10];
    let mut leg_free = 0usize;
    let mut leg_travel_empty = 0usize;
    let mut leg_loading = 0usize;
    let mut leg_travel_to_queue = 0usize;
    let mut leg_queuing = 0usize;
    let mut leg_travel_loaded = 0usize;
    let mut leg_unloading = 0usize;
    let mut leg_charging = 0usize;

    for (agent, index, heat_state, action_stats, is_dead, has_latency) in &agents_query {
        match &agent.task_leg {
            TaskLeg::Free => leg_free += 1,
            TaskLeg::TravelEmpty(_) => leg_travel_empty += 1,
            TaskLeg::Loading(_) => leg_loading += 1,
            TaskLeg::TravelToQueue { .. } => leg_travel_to_queue += 1,
            TaskLeg::Queuing { .. } => leg_queuing += 1,
            TaskLeg::TravelLoaded { .. } => leg_travel_loaded += 1,
            TaskLeg::Unloading { .. } => leg_unloading += 1,
            TaskLeg::Charging => leg_charging += 1,
        }
        total += 1;
        if is_dead {
            dead += 1;
        } else {
            alive += 1;
        }

        let heat = heat_state.map_or(0.0, |h| h.heat);
        // heat is already a 0-1 Weibull CDF stress indicator in the new model
        let heat_normalized = heat.clamp(0.0, 1.0);

        heat_sum += heat_normalized;
        if heat_normalized > max_heat {
            max_heat = heat_normalized;
        }
        let bucket = ((heat_normalized * 10.0) as usize).min(9);
        heat_histogram[bucket] += 1;
        idle_sum += action_stats.map_or(0.0, |s| s.idle_ratio());

        if !use_aggregate {
            let dist = (agent.current_pos.x - agent.goal_pos.x).abs()
                + (agent.current_pos.y - agent.goal_pos.y).abs();
            agents_data.push(AgentSnapshot {
                id: index.0,
                pos: [agent.current_pos.x, agent.current_pos.y],
                goal: [agent.goal_pos.x, agent.goal_pos.y],
                heat,
                heat_normalized,
                is_dead,
                has_latency,
                path_length: agent.path_length,
                distance_to_goal: dist,
                task_leg: agent.task_leg.label().to_string(),
                idle_ratio: action_stats.map_or(0.0, |s| s.idle_ratio()),
            });
        }
    }

    if !use_aggregate {
        agents_data.sort_by_key(|a| a.id);
    }

    let agent_summary = if use_aggregate {
        Some(AgentSummary {
            total,
            alive,
            dead,
            avg_heat: if total > 0 { heat_sum / total as f32 } else { 0.0 },
            max_heat,
            avg_idle_ratio: if total > 0 { idle_sum / total as f32 } else { 0.0 },
            heat_histogram,
        })
    } else {
        None
    };

    // Fill task_idle_count now that the agent loop has run
    if let Some(ref mut m) = metrics {
        m.task_idle_count = leg_free;
    }

    let info = solver.solver().info();
    let solver_info = SolverInfoSnapshot {
        optimality: info.optimality.label().to_string(),
        complexity: info.complexity.to_string(),
        scalability: info.scalability.label().to_string(),
        description: info.description.to_string(),
        source: info.source.to_string(),
        recommended_max_agents: info.recommended_max_agents,
    };

    let output = BridgeOutput {
        version: crate::constants::VERSION,
        state: state_str.to_string(),
        tick: config.tick,
        duration: config.duration,
        tick_hz: config.tick_hz,
        num_agents: ui_state.num_agents,
        seed: ui_state.seed,
        obstacle_density: ui_state.obstacle_density,
        grid_width: ui_state.grid_width,
        grid_height: ui_state.grid_height,
        total_agents: total,
        alive_agents: alive,
        dead_agents: dead,
        solver: ui_state.solver_name.clone(),
        solver_info,
        fault_config: FaultConfigSnapshot {
            enabled: fault_config.enabled,
            weibull_enabled: fault_config.weibull_enabled,
            weibull_beta: fault_config.weibull_beta,
            weibull_eta: fault_config.weibull_eta,
            intermittent_enabled: fault_config.intermittent_enabled,
            intermittent_mtbf_ticks: fault_config.intermittent_mtbf_ticks,
            intermittent_recovery_ticks: fault_config.intermittent_recovery_ticks,
        },
        analysis_config: AnalysisConfigSnapshot {
            heatmap_visible: analysis_config.heatmap_visible,
            heatmap_mode: match analysis.heatmap_state.mode {
                HeatmapMode::Density => "density",
                HeatmapMode::Traffic => "traffic",
                HeatmapMode::Criticality => "criticality",
            }
            .to_string(),
            heatmap_density_radius: analysis.heatmap_state.density_radius,
            path_visible: analysis_config.path_visible,
        },
        metrics_config: MetricsConfigSnapshot {
            aet: phase_res.metrics_config.aet,
            makespan: phase_res.metrics_config.makespan,
            mttr: phase_res.metrics_config.mttr,
            fault_count: phase_res.metrics_config.fault_count,
            cascade_depth: phase_res.metrics_config.cascade_depth,
            cascade_cost: phase_res.metrics_config.cascade_cost,
            fault_mttr: phase_res.metrics_config.fault_mttr,
            recovery_rate: phase_res.metrics_config.recovery_rate,
            cascade_spread: phase_res.metrics_config.cascade_spread,
            throughput: phase_res.metrics_config.throughput,
            idle_ratio: phase_res.metrics_config.idle_ratio,
        },
        export_config: ExportConfigSnapshot {
            auto_on_finished: export_config.auto_on_finished,
            auto_on_fault: export_config.auto_on_fault,
            periodic_enabled: export_config.periodic_enabled,
            periodic_interval: export_config.periodic_interval,
            export_json: export_config.export_json,
            export_csv: export_config.export_csv,
        },
        metrics,
        agents: agents_data,
        agent_summary,
        fps: fps_tracker.smoothed,
        max_agents: constants::MAX_AGENTS,
        map_capacity: phase_res.grid.walkable_count(),
        max_grid_dim: constants::MAX_GRID_DIM,
        scenario_loaded: ui_state.imported_scenario.is_some(),
        scenario_name: ui_state.imported_scenario.as_ref().map(|s| s.name.clone()),
        robot_opacity: robot_opacity.opacity,
        camera_mode: match phase_res.orbit.mode {
            CameraMode::Perspective => "3d",
            CameraMode::Orthographic => "2d",
        }
        .to_string(),
        graphics: GraphicsSnapshot {
            shadows: phase_res.graphics.shadows,
            msaa: phase_res.graphics.msaa,
            colorblind: phase_res.graphics.colorblind,
        },
        topology: phase_res.active_topology.name().to_string(),
        storage_rows: 0,
        task_leg_counts: TaskLegCounts {
            free: leg_free,
            travel_empty: leg_travel_empty,
            loading: leg_loading,
            travel_to_queue: leg_travel_to_queue,
            queuing: leg_queuing,
            travel_loaded: leg_travel_loaded,
            unloading: leg_unloading,
            charging: leg_charging,
        },
        lifelong: LifelongSnapshot {
            enabled: phase_res.lifelong.enabled,
            tasks_completed: phase_res.lifelong.tasks_completed,
            throughput: phase_res.lifelong.throughput(config.tick),
            scheduler: phase_res.active_scheduler.name().to_string(),
        },
        phase: PhaseSnapshot {
            phase: phase_res.phase.label().to_string(),
            baseline_throughput: phase_res.baseline.baseline_throughput,
            baseline_idle_ratio: phase_res.baseline.baseline_idle_ratio,
        },
        loading: if current == SimState::Loading {
            let total = loading_progress.total.max(1);
            Some(LoadingSnapshot {
                phase: match loading_progress.phase {
                    LoadingPhase::Setup => "setup".to_string(),
                    LoadingPhase::Obstacles => "obstacles".to_string(),
                    LoadingPhase::Agents => "agents".to_string(),
                    LoadingPhase::Baseline => "baseline".to_string(),
                    LoadingPhase::Solving => "solving".to_string(),
                    LoadingPhase::Done => "done".to_string(),
                },
                current: loading_progress.current,
                total: loading_progress.total,
                percent: (loading_progress.current as f32 / total as f32 * 100.0).min(100.0),
            })
        } else {
            None
        },
        fault_events: analysis
            .fault_metrics
            .event_records
            .iter()
            .rev()
            .take(100)
            .rev()
            .map(|r| FaultEventSnapshot {
                id: r.id,
                tick: r.tick,
                fault_type: r.fault_type.label(),
                source: r.source.label(),
                position: [r.position.x, r.position.y],
                agents_affected: r.agents_affected,
                cascade_depth: r.cascade_depth,
                throughput_before: r.throughput_before,
                throughput_min: r.throughput_min,
                throughput_delta: r.throughput_delta,
                recovered: r.recovered,
                recovery_tick: r.recovery_tick,
            })
            .collect(),
        scorecard: if phase_res.phase.is_fault_injection() {
            Some(ScorecardSnapshot {
                fault_tolerance: analysis.scorecard.fault_tolerance,
                nrr: analysis.scorecard.nrr,
                fleet_utilization: analysis.scorecard.fleet_utilization,
                critical_time: analysis.scorecard.critical_time,
                has_faults: analysis.scorecard.has_faults,
            })
        } else {
            None
        },
        replay: if current == SimState::Replay || !analysis.tick_history.snapshots.is_empty() {
            let cursor_idx = analysis.tick_history.replay_cursor.unwrap_or(0);
            let cursor_tick = analysis.tick_history.snapshots.get(cursor_idx).map_or(0, |s| s.tick);
            let max_tick = analysis.tick_history.snapshots.back().map_or(0, |s| s.tick);
            Some(ReplaySnapshot {
                cursor_tick,
                cursor_index: cursor_idx,
                total_recorded: analysis.tick_history.snapshots.len(),
                max_tick,
            })
        } else {
            None
        },
        click_selection: if click_selection.fresh {
            click_selection.fresh = false;
            Some(ClickSelectionSnapshot {
                agent_index: click_selection.agent_index,
                cell: click_selection.cell.map(|c| [c.x, c.y]),
                screen_x: click_selection.screen_x,
                screen_y: click_selection.screen_y,
            })
        } else {
            None
        },
        fault_scenario: {
            let sc = &phase_res.fault_scenario;
            FaultScenarioSnapshot {
                enabled: sc.enabled,
                scenario_type: sc.scenario_type.id().to_string(),
                label: sc.scenario_type.label().to_string(),
                needs_baseline: sc.needs_baseline(),
                burst_kill_percent: sc.burst_kill_percent,
                burst_at_tick: sc.burst_at_tick,
                wear_heat_rate: sc.wear_heat_rate.id().to_string(),
                wear_threshold: sc.wear_threshold,
                zone_at_tick: sc.zone_at_tick,
                zone_latency_duration: sc.zone_latency_duration,
                perm_zone_at_tick: sc.perm_zone_at_tick,
                perm_zone_block_percent: sc.perm_zone_block_percent,
            }
        },
        baseline_diff: {
            let store = &phase_res.baseline_store;
            let diff = &phase_res.baseline_diff;
            if let Some(ref record) = store.record {
                let tick = config.tick; // 1-based; accessors handle conversion
                let bl_throughput = record.throughput_at(tick);
                let live_throughput = phase_res.lifelong.throughput(config.tick);
                let live_tasks = phase_res.lifelong.tasks_completed;

                // Periodic diagnostic: log baseline vs live (debug builds only)
                #[cfg(all(target_arch = "wasm32", debug_assertions))]
                if current == SimState::Running && config.tick > 0 && config.tick % 50 == 0 {
                    let bl_tasks = record.tasks_at(tick);
                    let bl_idle = record.idle_at(tick);
                    let msg = format!(
                        "[PARITY t={}] bl_tp={:.0} live_tp={:.0} | bl_tasks={} live_tasks={} | bl_idle={} live_idle={} | gap={}",
                        config.tick,
                        bl_throughput,
                        live_throughput,
                        bl_tasks,
                        live_tasks,
                        bl_idle,
                        leg_free,
                        diff.gap,
                    );
                    web_sys::console::log_1(&msg.into());
                }

                BaselineDiffSnapshot {
                    has_baseline: true,
                    computing: false,
                    baseline_throughput: bl_throughput,
                    throughput_delta: live_throughput - bl_throughput,
                    tasks_delta: live_tasks as i64 - record.total_tasks as i64,
                    baseline_total_tasks: record.total_tasks,
                    baseline_tasks_at_tick: record.tasks_at(tick),
                    baseline_idle_at_tick: record.idle_at(tick),
                    baseline_wait_ratio_at_tick: record.wait_ratio_at(tick),
                    baseline_avg_throughput: record.avg_throughput,
                    gap: diff.gap,
                    deficit_integral: diff.deficit_integral,
                    surplus_integral: diff.surplus_integral,
                    net_integral: diff.net_integral,
                    impacted_area: diff.impacted_area,
                    rate_delta: diff.rate_delta,
                    recovery_tick: diff.recovery_tick,
                }
            } else {
                BaselineDiffSnapshot {
                    has_baseline: false,
                    computing: store.computing,
                    baseline_throughput: 0.0,
                    throughput_delta: 0.0,
                    tasks_delta: 0,
                    baseline_total_tasks: 0,
                    baseline_tasks_at_tick: 0,
                    baseline_idle_at_tick: 0,
                    baseline_wait_ratio_at_tick: 0.0,
                    baseline_avg_throughput: 0.0,
                    gap: 0,
                    deficit_integral: 0,
                    surplus_integral: 0,
                    net_integral: 0,
                    impacted_area: 0.0,
                    rate_delta: 0.0,
                    recovery_tick: None,
                }
            }
        },
        fault_schedule_markers: phase_res
            .fault_schedule
            .events
            .iter()
            .map(|ev| {
                let (marker_type, label) = match &ev.action {
                    ScheduledAction::KillRandomAgents(n) => {
                        ("burst_failure".to_string(), format!("Burst Failure — kill {} robots", n))
                    }
                    ScheduledAction::ZoneLatency { duration } => (
                        "zone_outage".to_string(),
                        format!("Zone Outage — {} tick latency", duration),
                    ),
                    ScheduledAction::ZoneBlock { block_percent } => (
                        "permanent_zone_outage".to_string(),
                        format!("Permanent Zone Outage — {}% cells blocked", *block_percent as u32),
                    ),
                };
                ScheduleMarkerSnapshot { tick: ev.tick, marker_type, label, fired: ev.fired }
            })
            .collect(),
    };

    #[cfg(target_arch = "wasm32")]
    {
        match serde_json::to_string(&output) {
            Ok(json) => {
                super::BRIDGE.with(|b| {
                    b.borrow_mut().outgoing = Some(json);
                });
            }
            Err(e) => {
                web_sys::console::error_1(&format!("Bridge serialize error: {e}").into());
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = output;
}
