use bevy::prelude::*;
use serde::Serialize;

use crate::analysis::{AnalysisConfig, MetricsConfig, TimeSeriesAccessor};
use crate::analysis::cascade::CascadeState;
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::history::TickHistory;
use crate::analysis::scorecard::ResilienceScorecard;
use crate::analysis::heatmap::{HeatmapMode, HeatmapState};
use crate::analysis::metrics::SimMetrics;
use crate::core::agent::{AgentActionStats, AgentIndex, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::state::{LoadingPhase, LoadingProgress, ResumeTarget, SimState, SimulationConfig, StepMode};
use crate::core::phase::{ResilienceBaseline, SimulationPhase};
use crate::core::queue::ActiveQueuePolicy;
use crate::core::task::{ActiveScheduler, LifelongConfig, TaskLeg};
use crate::core::topology::ActiveTopology;
use crate::export::config::{ExportConfig, ExportRequest};
use crate::analysis::baseline::BaselineStore;
use crate::fault::breakdown::{Dead, LatencyFault};
use crate::fault::config::FaultConfig;
use crate::fault::heat::HeatState;
use crate::fault::manual::ManualFaultCommand;
use crate::fault::scenario::{FaultScenario, FaultSchedule, ScheduledAction};
use crate::render::animator::RobotOpacity;
use crate::render::picking::ClickSelection;
use crate::render::graphics::GraphicsConfig;
use crate::render::orbit_camera::{CameraMode, OrbitCamera};
use crate::constants;
use crate::solver::ActiveSolver;

use super::controls::UiState;

#[cfg(target_arch = "wasm32")]
use crate::export::config::ExportTrigger;
#[cfg(target_arch = "wasm32")]
use crate::render::orbit_camera as orbit_camera_fns;
#[cfg(target_arch = "wasm32")]
use crate::solver::lifelong_solver_from_name;
#[cfg(target_arch = "wasm32")]
use super::controls::ImportedScenario;
#[cfg(target_arch = "wasm32")]
use super::controls::PreviewMap;
#[cfg(target_arch = "wasm32")]
use crate::core::topology::{CustomMap, ZoneMap, ZoneType};

// ---------------------------------------------------------------------------
// Thread-local bridge state (Bevy ↔ JS)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
struct BridgeInner {
    outgoing: Option<String>,
    incoming: Vec<JsCommand>,
}

#[cfg(target_arch = "wasm32")]
impl Default for BridgeInner {
    fn default() -> Self {
        Self {
            outgoing: None,
            incoming: Vec::new(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static BRIDGE: RefCell<BridgeInner> = RefCell::new(BridgeInner::default());
}

// ---------------------------------------------------------------------------
// JS commands
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone)]
enum JsCommand {
    SetState(String),
    SetTickHz(f64),
    SetNumAgents(usize),
    SetSeed(u64),
    SetObstacleDensity(f32),
    SetFaultEnabled(bool),
    SetFaultParam { key: String, value: f64 },
    SetAnalysisParam { key: String, value: bool },
    SetMetric { key: String, value: bool },
    SetExportParam { key: String, value: String },
    ExportNow,
    SetCameraPreset(String),
    Step,
    SetGridWidth(i32),
    SetGridHeight(i32),
    SetHeatmapMode(String),
    SetDensityRadius(i32),
    SetSolver(String),
    KillAgent(usize),
    PlaceObstacle { x: i32, y: i32 },
    InjectLatency { agent_id: usize, duration: u32 },
    LoadScenario(String),   // JSON payload with map + agents
    ClearScenario,
    LoadCustomMap(String),  // JSON from map maker (cells + robots + seed)
    SetDuration(u64),
    SetScheduler(String),
    SetTopology(String),
    SetPathVisible(bool),
    SetRobotOpacity(f32),
    SetCameraMode(String),
    SetGraphics { key: String, value: bool },
    SeekToTick(u64),
    StepBackward,
    JumpPrevFault,
    JumpNextFault,
    ClearSelection,
    SelectAgent(usize),
    SetRhcrHorizon(usize),
    SetRhcrReplanInterval(usize),
    SetRhcrFallback(String),
    SetQueuePolicy(String),
    SetFaultList(String),
    SetTheme(String),
}

#[cfg(target_arch = "wasm32")]
fn parse_command(json: &str) -> Option<JsCommand> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let cmd_type = v.get("type")?.as_str()?;

    match cmd_type {
        "set_state" => Some(JsCommand::SetState(v.get("value")?.as_str()?.to_string())),
        "set_tick_hz" => Some(JsCommand::SetTickHz(v.get("value")?.as_f64()?)),
        "set_num_agents" => Some(JsCommand::SetNumAgents(v.get("value")?.as_u64()? as usize)),
        "set_seed" => Some(JsCommand::SetSeed(v.get("value")?.as_u64()?)),
        "set_obstacle_density" => {
            Some(JsCommand::SetObstacleDensity(v.get("value")?.as_f64()? as f32))
        }
        "set_fault_enabled" => Some(JsCommand::SetFaultEnabled(v.get("value")?.as_bool()?)),
        "set_fault_param" => Some(JsCommand::SetFaultParam {
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.as_f64()?,
        }),
        "set_analysis_param" => Some(JsCommand::SetAnalysisParam {
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.as_bool()?,
        }),
        "set_metric" => Some(JsCommand::SetMetric {
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.as_bool()?,
        }),
        "set_export_param" => Some(JsCommand::SetExportParam {
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.to_string(),
        }),
        "export_now" => Some(JsCommand::ExportNow),
        "set_camera_preset" => {
            Some(JsCommand::SetCameraPreset(v.get("value")?.as_str()?.to_string()))
        }
        "step" => Some(JsCommand::Step),
        "set_grid_width" => Some(JsCommand::SetGridWidth(v.get("value")?.as_i64()? as i32)),
        "set_grid_height" => Some(JsCommand::SetGridHeight(v.get("value")?.as_i64()? as i32)),
        "set_heatmap_mode" => {
            Some(JsCommand::SetHeatmapMode(v.get("value")?.as_str()?.to_string()))
        }
        "set_density_radius" => {
            Some(JsCommand::SetDensityRadius(v.get("value")?.as_i64()? as i32))
        }
        "set_solver" => {
            Some(JsCommand::SetSolver(v.get("value")?.as_str()?.to_string()))
        }
        "kill_agent" => {
            let id = v.get("value")
                .and_then(|val| val.as_u64().or_else(|| val.as_str().and_then(|s| s.parse().ok())))
                ? as usize;
            Some(JsCommand::KillAgent(id))
        }
        "place_obstacle" => {
            let x = v.get("x")?.as_i64()? as i32;
            let y = v.get("y")?.as_i64()? as i32;
            Some(JsCommand::PlaceObstacle { x, y })
        }
        "inject_latency" => {
            let agent_id = v.get("value")?.as_u64()? as usize;
            let duration = v.get("duration").and_then(|d| d.as_u64()).unwrap_or(0) as u32;
            Some(JsCommand::InjectLatency { agent_id, duration })
        }
        "load_scenario" => {
            // Entire JSON value as string — we'll parse the fields in the handler
            Some(JsCommand::LoadScenario(json.to_string()))
        }
        "clear_scenario" => Some(JsCommand::ClearScenario),
        "load_custom_map" => {
            Some(JsCommand::LoadCustomMap(json.to_string()))
        }
        "set_duration" => {
            Some(JsCommand::SetDuration(v.get("value")?.as_u64()?))
        }
        "set_scheduler" => {
            Some(JsCommand::SetScheduler(v.get("value")?.as_str()?.to_string()))
        }
        "set_topology" => {
            Some(JsCommand::SetTopology(v.get("value")?.as_str()?.to_string()))
        }
        "set_path_visible" => Some(JsCommand::SetPathVisible(v.get("value")?.as_bool()?)),
        "set_robot_opacity" => {
            Some(JsCommand::SetRobotOpacity(v.get("value")?.as_f64()? as f32))
        }
        "set_camera_mode" => {
            Some(JsCommand::SetCameraMode(v.get("value")?.as_str()?.to_string()))
        }
        "set_graphics" => {
            Some(JsCommand::SetGraphics {
                key: v.get("key")?.as_str()?.to_string(),
                value: v.get("value")?.as_bool()?,
            })
        }
        "seek_to_tick" => Some(JsCommand::SeekToTick(v.get("value")?.as_u64()?)),
        "step_backward" => Some(JsCommand::StepBackward),
        "jump_prev_fault" => Some(JsCommand::JumpPrevFault),
        "jump_next_fault" => Some(JsCommand::JumpNextFault),
        "clear_selection" => Some(JsCommand::ClearSelection),
        "select_agent" => {
            let id = v.get("value")?.as_u64()? as usize;
            Some(JsCommand::SelectAgent(id))
        }
        "set_rhcr_horizon" => {
            Some(JsCommand::SetRhcrHorizon(v.get("value")?.as_u64()? as usize))
        }
        "set_rhcr_replan_interval" => {
            Some(JsCommand::SetRhcrReplanInterval(v.get("value")?.as_u64()? as usize))
        }
        "set_rhcr_fallback" => {
            Some(JsCommand::SetRhcrFallback(v.get("value")?.as_str()?.to_string()))
        }
        "set_queue_policy" => {
            Some(JsCommand::SetQueuePolicy(v.get("value")?.as_str()?.to_string()))
        }
        "set_fault_list" => {
            Some(JsCommand::SetFaultList(v.get("value")?.as_str()?.to_string()))
        }
        "set_theme" => {
            Some(JsCommand::SetTheme(v.get("value")?.as_str()?.to_string()))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// wasm_bindgen exports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn get_simulation_state() -> String {
    BRIDGE.with(|b| {
        let inner = b.borrow();
        inner.outgoing.clone().unwrap_or_else(|| "{}".to_string())
    })
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn send_command(cmd: &str) {
    if let Some(parsed) = parse_command(cmd) {
        BRIDGE.with(|b| {
            let mut inner = b.borrow_mut();
            if inner.incoming.len() < crate::constants::MAX_COMMAND_QUEUE {
                inner.incoming.push(parsed);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Experiment API (WASM)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn experiment_start() {
    crate::experiment::runner::wasm_experiment_start();
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn experiment_run_single(config_json: &str) -> String {
    crate::experiment::runner::wasm_experiment_run_single(config_json)
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn experiment_finish() -> String {
    crate::experiment::runner::wasm_experiment_finish()
}

// ---------------------------------------------------------------------------
// Serializable output for JS
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BridgeOutput {
    version: &'static str,
    state: String,
    tick: u64,
    duration: u64,
    tick_hz: f64,
    num_agents: usize,
    seed: u64,
    obstacle_density: f32,
    grid_width: i32,
    grid_height: i32,
    total_agents: usize,
    alive_agents: usize,
    dead_agents: usize,
    fault_config: FaultConfigSnapshot,
    analysis_config: AnalysisConfigSnapshot,
    metrics_config: MetricsConfigSnapshot,
    export_config: ExportConfigSnapshot,
    metrics: Option<MetricsSnapshot>,
    solver: String,
    solver_info: SolverInfoSnapshot,
    agents: Vec<AgentSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_summary: Option<AgentSummary>,
    fps: f32,
    max_agents: usize,
    map_capacity: usize,
    max_grid_dim: i32,
    scenario_loaded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    scenario_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loading: Option<LoadingSnapshot>,
    robot_opacity: f32,
    camera_mode: String,
    graphics: GraphicsSnapshot,
    topology: String,
    storage_rows: usize,
    task_leg_counts: TaskLegCounts,
    lifelong: LifelongSnapshot,
    phase: PhaseSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay: Option<ReplaySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scorecard: Option<ScorecardSnapshot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fault_events: Vec<FaultEventSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    click_selection: Option<ClickSelectionSnapshot>,
    fault_scenario: FaultScenarioSnapshot,
    baseline_diff: BaselineDiffSnapshot,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fault_schedule_markers: Vec<ScheduleMarkerSnapshot>,
}

#[derive(Serialize)]
struct ClickSelectionSnapshot {
    agent_index: Option<usize>,
    cell: Option<[i32; 2]>,
    screen_x: f32,
    screen_y: f32,
}

#[derive(Serialize)]
struct ReplaySnapshot {
    cursor_tick: u64,
    cursor_index: usize,
    total_recorded: usize,
    max_tick: u64,
}

#[derive(Serialize)]
struct FaultEventSnapshot {
    id: usize,
    tick: u64,
    fault_type: String,
    source: String,
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
struct ScorecardSnapshot {
    fault_tolerance: f32,
    nrr: Option<f32>,
    fleet_utilization: f32,
    critical_time: f32,
    has_faults: bool,
}

#[derive(Serialize)]
struct FaultScenarioSnapshot {
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
struct BaselineDiffSnapshot {
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
struct ScheduleMarkerSnapshot {
    tick: u64,
    marker_type: String,
    label: String,
    fired: bool,
}

#[derive(Serialize)]
struct LifelongSnapshot {
    enabled: bool,
    tasks_completed: u64,
    throughput: f64,
    scheduler: String,
}

#[derive(Serialize)]
struct TaskLegCounts {
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
struct LoadingSnapshot {
    phase: String,
    current: usize,
    total: usize,
    percent: f32,
}

#[derive(Serialize)]
struct AgentSummary {
    total: usize,
    alive: usize,
    dead: usize,
    avg_heat: f32,
    max_heat: f32,
    avg_idle_ratio: f32,
    heat_histogram: [u32; 10],
}

#[derive(Serialize)]
struct SolverInfoSnapshot {
    optimality: String,
    complexity: String,
    scalability: String,
    description: String,
    source: String,
    recommended_max_agents: Option<usize>,
}

#[derive(Serialize)]
struct AgentSnapshot {
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
struct FaultConfigSnapshot {
    enabled: bool,
    weibull_enabled: bool,
    weibull_beta: f32,
    weibull_eta: f32,
    intermittent_enabled: bool,
    intermittent_mtbf_ticks: u64,
    intermittent_recovery_ticks: u32,
}

#[derive(Serialize)]
struct AnalysisConfigSnapshot {
    heatmap_visible: bool,
    heatmap_mode: String,
    heatmap_density_radius: i32,
    path_visible: bool,
}

#[derive(Serialize)]
struct MetricsConfigSnapshot {
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
struct ExportConfigSnapshot {
    auto_on_finished: bool,
    auto_on_fault: bool,
    periodic_enabled: bool,
    periodic_interval: u64,
    export_json: bool,
    export_csv: bool,
}

#[derive(Serialize)]
struct PhaseSnapshot {
    phase: String,
    baseline_throughput: f64,
    baseline_idle_ratio: f32,
}

#[derive(Serialize)]
struct GraphicsSnapshot {
    shadows: bool,
    msaa: bool,
    colorblind: bool,
}

#[derive(Serialize)]
struct MetricsSnapshot {
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
// FPS tracker
// ---------------------------------------------------------------------------

#[derive(Resource)]
struct FpsTracker {
    smoothed: f32,
}

impl Default for FpsTracker {
    fn default() -> Self {
        Self { smoothed: 60.0 }
    }
}

// ---------------------------------------------------------------------------
// Bevy systems
// ---------------------------------------------------------------------------

use bevy::ecs::system::SystemParam;

#[derive(SystemParam)]
struct AnalysisResources<'w> {
    sim_metrics: Res<'w, SimMetrics>,
    cascade: Res<'w, CascadeState>,
    heatmap_state: Res<'w, HeatmapState>,
    fault_metrics: Res<'w, FaultMetrics>,
    tick_history: Res<'w, TickHistory>,
    scorecard: Res<'w, ResilienceScorecard>,
}

#[derive(SystemParam)]
struct PhaseResources<'w> {
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
    resume_target: Res<'w, ResumeTarget>,
    baseline_diff: Res<'w, crate::analysis::baseline::BaselineDiff>,
    grid: Res<'w, GridMap>,
}

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

fn sync_state_to_js(
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
        let latest_survival = analysis.fault_metrics
            .survival_series
            .back()
            .map(|(_, rate)| *rate)
            .unwrap_or(1.0);
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
        }.to_string(),
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
        fault_events: analysis.fault_metrics.event_records.iter().rev().take(100).rev().map(|r| {
            FaultEventSnapshot {
                id: r.id,
                tick: r.tick,
                fault_type: format!("{:?}", r.fault_type),
                source: format!("{:?}", r.source),
                position: [r.position.x, r.position.y],
                agents_affected: r.agents_affected,
                cascade_depth: r.cascade_depth,
                throughput_before: r.throughput_before,
                throughput_min: r.throughput_min,
                throughput_delta: r.throughput_delta,
                recovered: r.recovered,
                recovery_tick: r.recovery_tick,
            }
        }).collect(),
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
                        config.tick, bl_throughput, live_throughput,
                        bl_tasks, live_tasks,
                        bl_idle, leg_free,
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
        fault_schedule_markers: phase_res.fault_schedule.events.iter().map(|ev| {
            let (marker_type, label) = match &ev.action {
                ScheduledAction::KillRandomAgents(n) => (
                    "burst_failure".to_string(),
                    format!("Burst Failure — kill {} robots", n),
                ),
                ScheduledAction::ZoneLatency { duration } => (
                    "zone_outage".to_string(),
                    format!("Zone Outage — {} tick latency", duration),
                ),
                ScheduledAction::ZoneBlock { block_percent } => (
                    "permanent_zone_outage".to_string(),
                    format!("Permanent Zone Outage — {}% cells blocked", *block_percent as u32),
                ),
            };
            ScheduleMarkerSnapshot {
                tick: ev.tick,
                marker_type,
                label,
                fired: ev.fired,
            }
        }).collect(),
    };

    #[cfg(target_arch = "wasm32")]
    {
        match serde_json::to_string(&output) {
            Ok(json) => {
                BRIDGE.with(|b| {
                    b.borrow_mut().outgoing = Some(json);
                });
            }
            Err(e) => {
                web_sys::console::error_1(
                    &format!("Bridge serialize error: {e}").into(),
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = output;
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(SystemParam)]
struct LifelongResources<'w> {
    scheduler: ResMut<'w, ActiveScheduler>,
    queue_policy: ResMut<'w, ActiveQueuePolicy>,
    topology: ResMut<'w, ActiveTopology>,
    topo_registry: Res<'w, crate::core::topology::TopologyRegistry>,
    robot_opacity: ResMut<'w, RobotOpacity>,
    graphics: ResMut<'w, GraphicsConfig>,
    metrics_config: ResMut<'w, MetricsConfig>,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(SystemParam)]
struct SimCommandResources<'w> {
    config: ResMut<'w, SimulationConfig>,
    ui_state: ResMut<'w, UiState>,
    fault_config: ResMut<'w, FaultConfig>,
    analysis_config: ResMut<'w, AnalysisConfig>,
    export_config: ResMut<'w, ExportConfig>,
    orbit: ResMut<'w, OrbitCamera>,
    grid: ResMut<'w, GridMap>,
    heatmap_state: ResMut<'w, HeatmapState>,
    solver: ResMut<'w, ActiveSolver>,
    tick_history: ResMut<'w, TickHistory>,
}

#[cfg(target_arch = "wasm32")]
fn update_topology_dimensions(_ui_state: &UiState, _topology: &mut ActiveTopology) {
    // Topology dimensions come from JSON files; no runtime generation needed.
}

#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables, unused_mut))]
fn process_js_commands(
    mut commands: Commands,
    mut next_state: ResMut<NextState<SimState>>,
    sim_state: Res<State<SimState>>,
    mut fixed_time: ResMut<Time<Fixed>>,
    mut export_requests: MessageWriter<ExportRequest>,
    mut step_mode: ResMut<StepMode>,
    mut lifelong_res: LifelongResources,
    mut manual_faults: MessageWriter<ManualFaultCommand>,
    mut sim_res: SimCommandResources,
    mut click_selection: ResMut<ClickSelection>,
    mut rewind_req: ResMut<crate::fault::manual::RewindRequest>,
    mut clear_color: ResMut<ClearColor>,
    mut fault_list: ResMut<crate::fault::scenario::FaultList>,
) {
    #[cfg(target_arch = "wasm32")]
    {
        let js_commands: Vec<JsCommand> = BRIDGE.with(|b| {
            let mut inner = b.borrow_mut();
            std::mem::take(&mut inner.incoming)
        });

        // Mutable so reset can update it mid-batch, allowing config
        // commands in the same batch to see the new Idle state.
        let mut current = *sim_state.get();

        for cmd in js_commands {
            match cmd {
                JsCommand::SetState(s) => match s.as_str() {
                    "start" if current == SimState::Idle => {
                        next_state.set(SimState::Loading);
                    }
                    "pause" if current == SimState::Running => {
                        next_state.set(SimState::Paused);
                    }
                    "resume" if current == SimState::Paused => {
                        next_state.set(SimState::Running);
                    }
                    "resume" if current == SimState::Replay => {
                        // Delegate heavy work (grid rebuild, agent restore, truncation)
                        // to the apply_rewind system.
                        if let Some(cursor_idx) = sim_res.tick_history.replay_cursor {
                            if let Some(snapshot) = sim_res.tick_history.snapshots.get(cursor_idx) {
                                rewind_req.pending = Some(
                                    crate::fault::manual::RewindKind::ResumeFromTick(snapshot.tick),
                                );
                            }
                        }
                    }
                    "replay" if current == SimState::Paused || current == SimState::Running || current == SimState::Finished => {
                        // Enter replay at the last recorded tick
                        if !sim_res.tick_history.snapshots.is_empty() {
                            sim_res.tick_history.replay_cursor = Some(sim_res.tick_history.snapshots.len() - 1);
                            next_state.set(SimState::Replay);
                        }
                    }
                    "reset" if current != SimState::Idle => {
                        next_state.set(SimState::Idle);
                        *click_selection = ClickSelection::default();
                        // Update local state so subsequent commands in this
                        // batch see Idle (e.g. reset + set_solver + set_fault).
                        current = SimState::Idle;
                    }
                    _ => {}
                },
                JsCommand::SetTickHz(hz) => {
                    let hz = hz.clamp(1.0, 30.0);
                    sim_res.config.tick_hz = hz;
                    *fixed_time = Time::<Fixed>::from_hz(hz);
                }
                JsCommand::SetNumAgents(n) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.num_agents = n.clamp(constants::MIN_AGENTS, constants::MAX_AGENTS);
                    }
                }
                JsCommand::SetSeed(s) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.seed = s.min(9999);
                    }
                }
                JsCommand::SetObstacleDensity(d) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.obstacle_density = d.clamp(0.0, 0.3);
                        update_topology_dimensions(&sim_res.ui_state, &mut lifelong_res.topology);
                    }
                }
                JsCommand::SetFaultEnabled(v) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.fault_enabled = v;
                    }
                }
                JsCommand::SetFaultParam { key, value } => {
                    if current == SimState::Idle {
                        match key.as_str() {
                            "weibull_beta" => {
                                sim_res.fault_config.weibull_beta = (value as f32).clamp(1.0, 5.0)
                            }
                            "weibull_eta" => {
                                sim_res.fault_config.weibull_eta = (value as f32).clamp(10.0, 5000.0)
                            }
                            "intermittent_mtbf_ticks" => {
                                sim_res.fault_config.intermittent_mtbf_ticks =
                                    (value as u64).clamp(10, 10000)
                            }
                            "intermittent_recovery_ticks" => {
                                sim_res.fault_config.intermittent_recovery_ticks =
                                    (value as u32).clamp(1, 200)
                            }
                            _ => {}
                        }
                    }
                }
                JsCommand::SetAnalysisParam { key, value } => {
                    match key.as_str() {
                        "heatmap_visible" => sim_res.analysis_config.heatmap_visible = value,
                        _ => {}
                    }
                }
                JsCommand::SetMetric { key, value } => {
                    let mc = &mut lifelong_res.metrics_config;
                    match key.as_str() {
                        "aet" => mc.aet = value,
                        "makespan" => mc.makespan = value,
                        "mttr" => mc.mttr = value,
                        "fault_count" => mc.fault_count = value,
                        "cascade_depth" => mc.cascade_depth = value,
                        "cascade_cost" => mc.cascade_cost = value,
                        "fault_mttr" => mc.fault_mttr = value,
                        "recovery_rate" => mc.recovery_rate = value,
                        "cascade_spread" => mc.cascade_spread = value,
                        "throughput" => mc.throughput = value,
                        "idle_ratio" => mc.idle_ratio = value,
                        _ => {}
                    }
                }
                JsCommand::SetExportParam { key, value } => {
                    match key.as_str() {
                        "auto_on_finished" => {
                            sim_res.export_config.auto_on_finished = value == "true";
                        }
                        "auto_on_fault" => {
                            sim_res.export_config.auto_on_fault = value == "true";
                        }
                        "periodic_enabled" => {
                            sim_res.export_config.periodic_enabled = value == "true";
                        }
                        "periodic_interval" => {
                            if let Ok(v) = value.parse::<u64>() {
                                sim_res.export_config.periodic_interval = v.max(1).min(1000);
                            }
                        }
                        "export_json" => {
                            sim_res.export_config.export_json = value == "true";
                        }
                        "export_csv" => {
                            sim_res.export_config.export_csv = value == "true";
                        }
                        _ => {}
                    }
                }
                JsCommand::ExportNow => {
                    if current != SimState::Idle {
                        export_requests.write(ExportRequest {
                            trigger: ExportTrigger::Manual,
                            json: sim_res.export_config.export_json,
                            csv: sim_res.export_config.export_csv,
                        });
                    }
                }
                JsCommand::SetCameraPreset(preset) => {
                    let cx = (sim_res.grid.width as f32 - 1.0) * 0.5;
                    let cz = (sim_res.grid.height as f32 - 1.0) * 0.5;
                    let grid_center = Vec3::new(cx, 0.0, cz);
                    match preset.as_str() {
                        "center" => {
                            sim_res.orbit.focus = grid_center;
                        }
                        other => {
                            let (yaw, pitch, distance) = match other {
                                "top" => orbit_camera_fns::preset_top(&sim_res.grid),
                                _ => orbit_camera_fns::preset_side(&sim_res.grid),
                            };
                            sim_res.orbit.focus = grid_center;
                            sim_res.orbit.target_yaw = yaw;
                            sim_res.orbit.target_pitch = pitch;
                            sim_res.orbit.target_distance = distance;
                            sim_res.orbit.animating = true;
                        }
                    }
                }
                JsCommand::Step => {
                    if current == SimState::Paused {
                        step_mode.pending = true;
                        next_state.set(SimState::Running);
                    } else if current == SimState::Replay {
                        // Step forward in replay
                        let snap_len = sim_res.tick_history.snapshots.len();
                        if let Some(ref mut cursor) = sim_res.tick_history.replay_cursor {
                            if *cursor + 1 < snap_len {
                                *cursor += 1;
                            }
                        }
                    }
                }
                JsCommand::SetGridWidth(w) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.grid_width = w.clamp(constants::MIN_GRID_DIM, constants::MAX_GRID_DIM);
                        // Rebuild topology with new dimensions
                        update_topology_dimensions(&sim_res.ui_state, &mut lifelong_res.topology);
                    }
                }
                JsCommand::SetGridHeight(h) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.grid_height = h.clamp(constants::MIN_GRID_DIM, constants::MAX_GRID_DIM);
                        update_topology_dimensions(&sim_res.ui_state, &mut lifelong_res.topology);
                    }
                }
                JsCommand::SetHeatmapMode(mode) => {
                    let new_mode = match mode.as_str() {
                        "traffic" => HeatmapMode::Traffic,
                        "criticality" => HeatmapMode::Criticality,
                        _ => HeatmapMode::Density,
                    };
                    sim_res.heatmap_state.mode = new_mode;
                    sim_res.heatmap_state.dirty = true;
                }
                JsCommand::SetDensityRadius(r) => {
                    sim_res.heatmap_state.density_radius = r.clamp(1, 3);
                }
                JsCommand::SetSolver(name) => {
                    if current == SimState::Idle {
                        let grid_area = (sim_res.grid.width * sim_res.grid.height) as usize;
                        let num_agents = sim_res.ui_state.num_agents;
                        if let Some(new_solver) = lifelong_solver_from_name(&name, grid_area, num_agents) {
                            sim_res.ui_state.solver_name = name.clone();
                            // Clear RHCR overrides when switching solver
                            sim_res.ui_state.rhcr_horizon = None;
                            sim_res.ui_state.rhcr_replan_interval = None;
                            sim_res.ui_state.rhcr_fallback = None;
                            *sim_res.solver = ActiveSolver::new(new_solver);
                        }
                    }
                }
                JsCommand::KillAgent(id) => {
                    if current != SimState::Idle {
                        manual_faults.write(ManualFaultCommand::KillAgent(id));
                    }
                }
                JsCommand::PlaceObstacle { x, y } => {
                    if current != SimState::Idle {
                        manual_faults.write(ManualFaultCommand::PlaceObstacle(IVec2::new(x, y)));
                    }
                }
                JsCommand::InjectLatency { agent_id, duration } => {
                    if current != SimState::Idle {
                        manual_faults.write(ManualFaultCommand::InjectLatency {
                            agent_id,
                            duration,
                        });
                    }
                }
                JsCommand::LoadScenario(json_str) => {
                    if current == SimState::Idle {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
                            let width = v.get("width").and_then(|w| w.as_i64()).unwrap_or(0) as i32;
                            let height = v.get("height").and_then(|h| h.as_i64()).unwrap_or(0) as i32;
                            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();

                            if width >= constants::MIN_GRID_DIM && width <= constants::MAX_GRID_DIM
                                && height >= constants::MIN_GRID_DIM && height <= constants::MAX_GRID_DIM
                            {
                                let mut obstacles = std::collections::HashSet::new();
                                if let Some(obs_arr) = v.get("obstacles").and_then(|o| o.as_array()) {
                                    for ob in obs_arr {
                                        if let Some(pair) = ob.as_array() {
                                            if pair.len() >= 2 {
                                                let x = pair[0].as_i64().unwrap_or(-1) as i32;
                                                let y = pair[1].as_i64().unwrap_or(-1) as i32;
                                                if x >= 0 && x < width && y >= 0 && y < height {
                                                    obstacles.insert(IVec2::new(x, y));
                                                }
                                            }
                                        }
                                    }
                                }

                                let mut agents = Vec::new();
                                if let Some(ag_arr) = v.get("agents").and_then(|a| a.as_array()) {
                                    for ag in ag_arr {
                                        let sx = ag.get("sx").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
                                        let sy = ag.get("sy").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
                                        let gx = ag.get("gx").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
                                        let gy = ag.get("gy").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
                                        let start = IVec2::new(sx, sy);
                                        let goal = IVec2::new(gx, gy);
                                        // Validate positions are in bounds and not obstacles
                                        if sx >= 0 && sx < width && sy >= 0 && sy < height
                                            && gx >= 0 && gx < width && gy >= 0 && gy < height
                                            && !obstacles.contains(&start)
                                            && !obstacles.contains(&goal)
                                        {
                                            agents.push((start, goal));
                                        }
                                    }
                                }

                                let agent_count = agents.len().min(constants::MAX_AGENTS);
                                agents.truncate(agent_count);

                                if !agents.is_empty() {
                                    let total_cells = (width * height) as f32;
                                    sim_res.ui_state.grid_width = width;
                                    sim_res.ui_state.grid_height = height;
                                    sim_res.ui_state.num_agents = agent_count;
                                    sim_res.ui_state.obstacle_density = obstacles.len() as f32 / total_cells;
                                    sim_res.ui_state.imported_scenario = Some(ImportedScenario {
                                        name,
                                        width,
                                        height,
                                        obstacles,
                                        agents,
                                    });
                                }
                            }
                        }
                    }
                }
                JsCommand::ClearScenario => {
                    if current == SimState::Idle {
                        sim_res.ui_state.imported_scenario = None;
                        // Reset to first available topology from registry
                        if let Some(entry) = lifelong_res.topo_registry.entries.first() {
                            sim_res.ui_state.topology_name = entry.id.clone();
                            sim_res.ui_state.grid_width = entry.width;
                            sim_res.ui_state.grid_height = entry.height;
                            sim_res.ui_state.num_agents = entry.number_agents;
                            if let Some(at) = ActiveTopology::from_entry(entry) {
                                *lifelong_res.topology = at;
                            }
                        }
                        sim_res.ui_state.obstacle_density = 0.15;
                    }
                }
                JsCommand::LoadCustomMap(json_str) => {
                    if current == SimState::Idle {
                        if let Some(preview) = parse_custom_map(&json_str) {
                            // Set topology to custom so begin_loading uses it
                            lifelong_res.topology.set(Box::new(CustomMap {
                                grid: GridMap::with_obstacles(
                                    preview.grid.width,
                                    preview.grid.height,
                                    preview.grid.obstacles().clone(),
                                ),
                                zones: preview.zones.clone(),
                            }));
                            sim_res.ui_state.topology_name = "custom".to_string();
                            sim_res.ui_state.imported_scenario = Some(ImportedScenario {
                                name: preview.name.clone(),
                                width: preview.grid.width,
                                height: preview.grid.height,
                                obstacles: preview.grid.obstacles().clone(),
                                agents: preview.robot_starts.iter()
                                    .map(|&pos| (pos, pos))
                                    .collect(),
                            });
                            // Insert PreviewMap resource to trigger 3D preview
                            commands.insert_resource(preview);
                        }
                    }
                }
                JsCommand::SetDuration(d) => {
                    if current == SimState::Idle {
                        sim_res.config.duration = d.clamp(
                            crate::constants::MIN_DURATION,
                            crate::constants::MAX_DURATION,
                        );
                    }
                }
                JsCommand::SetScheduler(name) => {
                    if current == SimState::Idle {
                        *lifelong_res.scheduler = ActiveScheduler::from_name(&name);
                    }
                }
                JsCommand::SetQueuePolicy(name) => {
                    if current == SimState::Idle {
                        *lifelong_res.queue_policy = ActiveQueuePolicy::from_name(&name);
                    }
                }
                JsCommand::SetTopology(name) => {
                    if current == SimState::Idle {
                        // Look up topology from registry
                        if let Some(entry) = lifelong_res.topo_registry.find(&name) {
                            sim_res.ui_state.grid_width = entry.width;
                            sim_res.ui_state.grid_height = entry.height;
                            sim_res.ui_state.num_agents = entry.number_agents;
                            if let Some(at) = ActiveTopology::from_entry(entry) {
                                *lifelong_res.topology = at;
                            }
                        }
                        sim_res.ui_state.topology_name = name;
                        // Clear any imported scenario — registry topology uses
                        // RNG-based placement, not fixed agent positions.
                        sim_res.ui_state.imported_scenario = None;
                    }
                }
                JsCommand::SetPathVisible(v) => {
                    sim_res.analysis_config.path_visible = v;
                }
                JsCommand::SetRobotOpacity(v) => {
                    lifelong_res.robot_opacity.opacity = v.clamp(0.1, 1.0);
                }
                JsCommand::SetCameraMode(mode) => {
                    if current == SimState::Idle {
                        match mode.as_str() {
                            "2d" => sim_res.orbit.mode = CameraMode::Orthographic,
                            _ => sim_res.orbit.mode = CameraMode::Perspective,
                        }
                    }
                }
                JsCommand::SetGraphics { key, value } => {
                    match key.as_str() {
                        "shadows" => lifelong_res.graphics.shadows = value,
                        "msaa" => lifelong_res.graphics.msaa = value,
                        "colorblind" => lifelong_res.graphics.colorblind = value,
                        _ => {}
                    }
                }
                JsCommand::SeekToTick(tick) => {
                    if current == SimState::Replay || current == SimState::Paused
                        || current == SimState::Running || current == SimState::Finished
                    {
                        if let Some(idx) = sim_res.tick_history.tick_to_index(tick) {
                            sim_res.tick_history.replay_cursor = Some(idx);
                            if current != SimState::Replay {
                                next_state.set(SimState::Replay);
                            }
                        }
                    }
                }
                JsCommand::StepBackward => {
                    if current == SimState::Replay {
                        if let Some(ref mut cursor) = sim_res.tick_history.replay_cursor {
                            *cursor = cursor.saturating_sub(1);
                        }
                    }
                }
                JsCommand::JumpPrevFault => {
                    if current == SimState::Replay {
                        let cursor = sim_res.tick_history.replay_cursor.unwrap_or(0);
                        if let Some(idx) = sim_res.tick_history.prev_fault_index(cursor) {
                            sim_res.tick_history.replay_cursor = Some(idx);
                        }
                    }
                }
                JsCommand::JumpNextFault => {
                    if current == SimState::Replay {
                        let cursor = sim_res.tick_history.replay_cursor.unwrap_or(0);
                        if let Some(idx) = sim_res.tick_history.next_fault_index(cursor) {
                            sim_res.tick_history.replay_cursor = Some(idx);
                        }
                    }
                }
                JsCommand::ClearSelection => {
                    *click_selection = ClickSelection::default();
                }
                JsCommand::SelectAgent(id) => {
                    click_selection.agent_index = Some(id);
                    click_selection.cell = None;
                    click_selection.fresh = false;
                }
                JsCommand::SetRhcrHorizon(h) => {
                    if current == SimState::Idle {
                        let h = h.clamp(
                            crate::constants::RHCR_MIN_HORIZON,
                            crate::constants::RHCR_MAX_HORIZON,
                        );
                        sim_res.ui_state.rhcr_horizon = Some(h);
                    }
                }
                JsCommand::SetRhcrReplanInterval(w) => {
                    if current == SimState::Idle {
                        let w = w.clamp(
                            crate::constants::RHCR_MIN_REPLAN_INTERVAL,
                            crate::constants::RHCR_MAX_REPLAN_INTERVAL,
                        );
                        sim_res.ui_state.rhcr_replan_interval = Some(w);
                    }
                }
                JsCommand::SetRhcrFallback(mode) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.rhcr_fallback = Some(mode);
                    }
                }
                JsCommand::SetFaultList(json) => {
                    if current == SimState::Idle {
                        if let Some(list) = parse_fault_list_json(&json) {
                            *fault_list = list;
                            sim_res.ui_state.fault_enabled = !fault_list.items.is_empty();
                        }
                    }
                }
                JsCommand::SetTheme(ref theme) => {
                    // Update ClearColor to match CSS theme
                    if theme == "dark" {
                        // Match dark mode --bg-body: rgb(18, 18, 22)
                        clear_color.0 = Color::srgb(0.071, 0.071, 0.086);
                    } else {
                        // White canvas background for light mode
                        clear_color.0 = Color::WHITE;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Custom map JSON parser
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
fn parse_custom_map(json_str: &str) -> Option<PreviewMap> {
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let width = v.get("width").and_then(|w| w.as_i64())? as i32;
    let height = v.get("height").and_then(|h| h.as_i64())? as i32;
    let seed = v.get("seed").and_then(|s| s.as_u64()).unwrap_or(42);
    let name = v.get("name").and_then(|n| n.as_str())
        .unwrap_or("Custom Map").to_string();

    if width < constants::MIN_GRID_DIM || width > constants::MAX_GRID_DIM
        || height < constants::MIN_GRID_DIM || height > constants::MAX_GRID_DIM
    {
        return None;
    }

    let mut obstacles = std::collections::HashSet::new();
    let mut zones = ZoneMap::default();
    let mut delivery_directions: Vec<(IVec2, crate::core::action::Direction)> = Vec::new();

    // Parse cells: only non-walkable cells are stored (sparse)
    if let Some(cells) = v.get("cells").and_then(|c| c.as_array()) {
        for cell in cells {
            let x = cell.get("x").and_then(|v| v.as_i64())? as i32;
            let y = cell.get("y").and_then(|v| v.as_i64())? as i32;
            let cell_type = cell.get("type").and_then(|v| v.as_str())?;
            let pos = IVec2::new(x, y);

            if x < 0 || x >= width || y < 0 || y >= height {
                continue;
            }

            match cell_type {
                "wall" => {
                    obstacles.insert(pos);
                }
                "pickup" => {
                    zones.pickup_cells.push(pos);
                    zones.zone_type.insert(pos, ZoneType::Pickup);
                }
                "delivery" => {
                    zones.delivery_cells.push(pos);
                    zones.zone_type.insert(pos, ZoneType::Delivery);
                    // Parse optional queue direction for delivery cells
                    if let Some(dir_str) = cell.get("queue_direction").and_then(|v| v.as_str()) {
                        let dir = match dir_str {
                            "north" => Some(crate::core::action::Direction::North),
                            "south" => Some(crate::core::action::Direction::South),
                            "east" => Some(crate::core::action::Direction::East),
                            "west" => Some(crate::core::action::Direction::West),
                            _ => None,
                        };
                        if let Some(d) = dir {
                            delivery_directions.push((pos, d));
                        }
                    }
                }
                "recharging" => {
                    zones.recharging_cells.push(pos);
                    zones.zone_type.insert(pos, ZoneType::Recharging);
                }
                _ => {}
            }
        }
    }

    // All walkable non-zone cells are corridors
    for x in 0..width {
        for y in 0..height {
            let pos = IVec2::new(x, y);
            if !obstacles.contains(&pos) && !zones.zone_type.contains_key(&pos) {
                zones.corridor_cells.push(pos);
                zones.zone_type.insert(pos, ZoneType::Corridor);
            }
        }
    }

    let grid = GridMap::with_obstacles(width, height, obstacles);

    // Build queue lines from delivery cells with directions
    if !delivery_directions.is_empty() {
        zones.queue_lines = crate::core::queue::build_queue_lines(&delivery_directions, &grid);
    }

    // Parse robot positions
    let mut robot_starts = Vec::new();
    if let Some(robots) = v.get("robots").and_then(|r| r.as_array()) {
        for robot in robots {
            let x = robot.get("x").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
            let y = robot.get("y").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
            let pos = IVec2::new(x, y);
            if grid.is_walkable(pos) {
                robot_starts.push(pos);
            }
        }
    }

    // Truncate to MAX_AGENTS
    robot_starts.truncate(constants::MAX_AGENTS);

    Some(PreviewMap {
        grid,
        zones,
        robot_starts,
        seed,
        name,
    })
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// SystemSet for bridge command processing — other systems that read
/// Messages written by the bridge should run `.after(BridgeSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeSet;

pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsTracker>()
            .add_systems(Update, (
                sync_state_to_js,
                process_js_commands.in_set(BridgeSet),
            ));
    }
}

// ---------------------------------------------------------------------------
// Parse fault list JSON from JS
// ---------------------------------------------------------------------------

fn parse_fault_list_json(json: &str) -> Option<crate::fault::scenario::FaultList> {
    use crate::fault::scenario::*;

    let arr: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let mut items = Vec::new();

    for v in &arr {
        let fault_type = match v.get("type")?.as_str()? {
            "burst_failure" => FaultScenarioType::BurstFailure,
            "wear_based" => FaultScenarioType::WearBased,
            "zone_outage" => FaultScenarioType::ZoneOutage,
            "intermittent_fault" => FaultScenarioType::IntermittentFault,
            "permanent_zone_outage" => FaultScenarioType::PermanentZoneOutage,
            _ => continue,
        };

        let mut item = FaultItem {
            fault_type,
            ..Default::default()
        };

        match fault_type {
            FaultScenarioType::BurstFailure => {
                item.burst_kill_percent = v.get("kill_percent")
                    .and_then(|v| v.as_f64()).unwrap_or(20.0) as f32;
                item.burst_at_tick = v.get("at_tick")
                    .and_then(|v| v.as_u64()).unwrap_or(100);
            }
            FaultScenarioType::WearBased => {
                let rate_str = v.get("heat_rate")
                    .and_then(|v| v.as_str()).unwrap_or("medium");
                item.wear_heat_rate = rate_str.parse().unwrap_or_default();
                // Custom Weibull override
                if let (Some(beta), Some(eta)) = (
                    v.get("custom_beta").and_then(|v| v.as_f64()),
                    v.get("custom_eta").and_then(|v| v.as_f64()),
                ) {
                    item.custom_weibull = Some((beta as f32, eta as f32));
                }
            }
            FaultScenarioType::ZoneOutage => {
                item.zone_at_tick = v.get("at_tick")
                    .and_then(|v| v.as_u64()).unwrap_or(100);
                item.zone_latency_duration = v.get("duration")
                    .and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            }
            FaultScenarioType::IntermittentFault => {
                item.intermittent_mtbf_ticks = v.get("mtbf")
                    .and_then(|v| v.as_u64()).unwrap_or(80);
                item.intermittent_recovery_ticks = v.get("recovery")
                    .and_then(|v| v.as_u64()).unwrap_or(15) as u32;
            }
            FaultScenarioType::PermanentZoneOutage => {
                item.perm_zone_at_tick = v.get("at_tick")
                    .and_then(|v| v.as_u64()).unwrap_or(100);
                item.perm_zone_block_percent = v.get("block_percent")
                    .and_then(|v| v.as_f64()).unwrap_or(100.0) as f32;
            }
        }

        items.push(item);
    }

    Some(FaultList { items })
}
