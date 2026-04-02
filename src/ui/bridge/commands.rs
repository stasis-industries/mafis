use bevy::prelude::*;
use bevy::ecs::system::SystemParam;

use crate::analysis::heatmap::{HeatmapMode, HeatmapState};
use crate::analysis::history::TickHistory;
use crate::analysis::{AnalysisConfig, MetricsConfig};
use crate::constants;
use crate::core::grid::GridMap;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::state::{SimState, SimulationConfig, StepMode};
use crate::core::task::ActiveScheduler;
use crate::core::topology::ActiveTopology;
use crate::export::config::{ExportConfig, ExportRequest};
use crate::fault::config::FaultConfig;
use crate::fault::manual::ManualFaultCommand;
use crate::render::animator::RobotOpacity;
use crate::render::graphics::GraphicsConfig;
use crate::render::orbit_camera::{CameraMode, OrbitCamera};
use crate::render::picking::ClickSelection;
use crate::solver::ActiveSolver;

use crate::ui::controls::UiState;

#[cfg(target_arch = "wasm32")]
use crate::ui::controls::ImportedScenario;
#[cfg(target_arch = "wasm32")]
use crate::ui::controls::PreviewMap;
#[cfg(target_arch = "wasm32")]
use crate::core::topology::{CustomMap, ZoneMap, ZoneType};
#[cfg(target_arch = "wasm32")]
use crate::export::config::ExportTrigger;
#[cfg(target_arch = "wasm32")]
use crate::render::orbit_camera as orbit_camera_fns;
#[cfg(target_arch = "wasm32")]
use crate::solver::lifelong_solver_from_name;

// ---------------------------------------------------------------------------
// JS commands
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone)]
pub(crate) enum JsCommand {
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
    LoadScenario(String), // JSON payload with map + agents
    ClearScenario,
    LoadCustomMap(String), // JSON from map maker (cells + robots + seed)
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
pub(super) fn parse_command(json: &str) -> Option<JsCommand> {
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
        "set_density_radius" => Some(JsCommand::SetDensityRadius(v.get("value")?.as_i64()? as i32)),
        "set_solver" => Some(JsCommand::SetSolver(v.get("value")?.as_str()?.to_string())),
        "kill_agent" => {
            let id = v.get("value").and_then(|val| {
                val.as_u64().or_else(|| val.as_str().and_then(|s| s.parse().ok()))
            })? as usize;
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
        "load_custom_map" => Some(JsCommand::LoadCustomMap(json.to_string())),
        "set_duration" => Some(JsCommand::SetDuration(v.get("value")?.as_u64()?)),
        "set_scheduler" => Some(JsCommand::SetScheduler(v.get("value")?.as_str()?.to_string())),
        "set_topology" => Some(JsCommand::SetTopology(v.get("value")?.as_str()?.to_string())),
        "set_path_visible" => Some(JsCommand::SetPathVisible(v.get("value")?.as_bool()?)),
        "set_robot_opacity" => Some(JsCommand::SetRobotOpacity(v.get("value")?.as_f64()? as f32)),
        "set_camera_mode" => Some(JsCommand::SetCameraMode(v.get("value")?.as_str()?.to_string())),
        "set_graphics" => Some(JsCommand::SetGraphics {
            key: v.get("key")?.as_str()?.to_string(),
            value: v.get("value")?.as_bool()?,
        }),
        "seek_to_tick" => Some(JsCommand::SeekToTick(v.get("value")?.as_u64()?)),
        "step_backward" => Some(JsCommand::StepBackward),
        "jump_prev_fault" => Some(JsCommand::JumpPrevFault),
        "jump_next_fault" => Some(JsCommand::JumpNextFault),
        "clear_selection" => Some(JsCommand::ClearSelection),
        "select_agent" => {
            let id = v.get("value")?.as_u64()? as usize;
            Some(JsCommand::SelectAgent(id))
        }
        "set_rhcr_horizon" => Some(JsCommand::SetRhcrHorizon(v.get("value")?.as_u64()? as usize)),
        "set_rhcr_replan_interval" => {
            Some(JsCommand::SetRhcrReplanInterval(v.get("value")?.as_u64()? as usize))
        }
        "set_rhcr_fallback" => {
            Some(JsCommand::SetRhcrFallback(v.get("value")?.as_str()?.to_string()))
        }
        "set_queue_policy" => {
            Some(JsCommand::SetQueuePolicy(v.get("value")?.as_str()?.to_string()))
        }
        "set_fault_list" => Some(JsCommand::SetFaultList(v.get("value")?.as_str()?.to_string())),
        "set_theme" => Some(JsCommand::SetTheme(v.get("value")?.as_str()?.to_string())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// SystemParam bundles for process_js_commands
// ---------------------------------------------------------------------------

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(SystemParam)]
pub(super) struct LifelongResources<'w> {
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
pub(super) struct SimCommandResources<'w> {
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

// ---------------------------------------------------------------------------
// process_js_commands system
// ---------------------------------------------------------------------------

#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables, unused_mut))]
pub(super) fn process_js_commands(
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
        let js_commands: Vec<JsCommand> = super::BRIDGE.with(|b| {
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
                    "replay"
                        if current == SimState::Paused
                            || current == SimState::Running
                            || current == SimState::Finished =>
                    {
                        // Enter replay at the last recorded tick
                        if !sim_res.tick_history.snapshots.is_empty() {
                            sim_res.tick_history.replay_cursor =
                                Some(sim_res.tick_history.snapshots.len() - 1);
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
                        sim_res.ui_state.num_agents =
                            n.clamp(constants::MIN_AGENTS, constants::MAX_AGENTS);
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
                                sim_res.fault_config.weibull_eta =
                                    (value as f32).clamp(10.0, 5000.0)
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
                JsCommand::SetAnalysisParam { key, value } => match key.as_str() {
                    "heatmap_visible" => sim_res.analysis_config.heatmap_visible = value,
                    _ => {}
                },
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
                JsCommand::SetExportParam { key, value } => match key.as_str() {
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
                },
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
                        sim_res.ui_state.grid_width =
                            w.clamp(constants::MIN_GRID_DIM, constants::MAX_GRID_DIM);
                        // Rebuild topology with new dimensions
                        update_topology_dimensions(&sim_res.ui_state, &mut lifelong_res.topology);
                    }
                }
                JsCommand::SetGridHeight(h) => {
                    if current == SimState::Idle {
                        sim_res.ui_state.grid_height =
                            h.clamp(constants::MIN_GRID_DIM, constants::MAX_GRID_DIM);
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
                        if let Some(new_solver) =
                            lifelong_solver_from_name(&name, grid_area, num_agents)
                        {
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
                        manual_faults
                            .write(ManualFaultCommand::InjectLatency { agent_id, duration });
                    }
                }
                JsCommand::LoadScenario(json_str) => {
                    if current == SimState::Idle {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
                            let width = v.get("width").and_then(|w| w.as_i64()).unwrap_or(0) as i32;
                            let height =
                                v.get("height").and_then(|h| h.as_i64()).unwrap_or(0) as i32;
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown")
                                .to_string();

                            if width >= constants::MIN_GRID_DIM
                                && width <= constants::MAX_GRID_DIM
                                && height >= constants::MIN_GRID_DIM
                                && height <= constants::MAX_GRID_DIM
                            {
                                let mut obstacles = std::collections::HashSet::new();
                                if let Some(obs_arr) = v.get("obstacles").and_then(|o| o.as_array())
                                {
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
                                        let sx = ag.get("sx").and_then(|v| v.as_i64()).unwrap_or(-1)
                                            as i32;
                                        let sy = ag.get("sy").and_then(|v| v.as_i64()).unwrap_or(-1)
                                            as i32;
                                        let gx = ag.get("gx").and_then(|v| v.as_i64()).unwrap_or(-1)
                                            as i32;
                                        let gy = ag.get("gy").and_then(|v| v.as_i64()).unwrap_or(-1)
                                            as i32;
                                        let start = IVec2::new(sx, sy);
                                        let goal = IVec2::new(gx, gy);
                                        // Validate positions are in bounds and not obstacles
                                        if sx >= 0
                                            && sx < width
                                            && sy >= 0
                                            && sy < height
                                            && gx >= 0
                                            && gx < width
                                            && gy >= 0
                                            && gy < height
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
                                    sim_res.ui_state.obstacle_density =
                                        obstacles.len() as f32 / total_cells;
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
                                agents: preview
                                    .robot_starts
                                    .iter()
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
                        sim_res.config.duration =
                            d.clamp(crate::constants::MIN_DURATION, crate::constants::MAX_DURATION);
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
                JsCommand::SetGraphics { key, value } => match key.as_str() {
                    "shadows" => lifelong_res.graphics.shadows = value,
                    "msaa" => lifelong_res.graphics.msaa = value,
                    "colorblind" => lifelong_res.graphics.colorblind = value,
                    _ => {}
                },
                JsCommand::SeekToTick(tick) => {
                    if current == SimState::Replay
                        || current == SimState::Paused
                        || current == SimState::Running
                        || current == SimState::Finished
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
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("Custom Map").to_string();

    if width < constants::MIN_GRID_DIM
        || width > constants::MAX_GRID_DIM
        || height < constants::MIN_GRID_DIM
        || height > constants::MAX_GRID_DIM
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

    Some(PreviewMap { grid, zones, robot_starts, seed, name })
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

        let mut item = FaultItem { fault_type, ..Default::default() };

        match fault_type {
            FaultScenarioType::BurstFailure => {
                item.burst_kill_percent =
                    v.get("kill_percent").and_then(|v| v.as_f64()).unwrap_or(20.0) as f32;
                item.burst_at_tick = v.get("at_tick").and_then(|v| v.as_u64()).unwrap_or(100);
            }
            FaultScenarioType::WearBased => {
                let rate_str = v.get("heat_rate").and_then(|v| v.as_str()).unwrap_or("medium");
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
                item.zone_at_tick = v.get("at_tick").and_then(|v| v.as_u64()).unwrap_or(100);
                item.zone_latency_duration =
                    v.get("duration").and_then(|v| v.as_u64()).unwrap_or(50) as u32;
            }
            FaultScenarioType::IntermittentFault => {
                item.intermittent_mtbf_ticks = v.get("mtbf").and_then(|v| v.as_u64()).unwrap_or(80);
                item.intermittent_recovery_ticks =
                    v.get("recovery").and_then(|v| v.as_u64()).unwrap_or(15) as u32;
            }
            FaultScenarioType::PermanentZoneOutage => {
                item.perm_zone_at_tick = v.get("at_tick").and_then(|v| v.as_u64()).unwrap_or(100);
                item.perm_zone_block_percent =
                    v.get("block_percent").and_then(|v| v.as_f64()).unwrap_or(100.0) as f32;
            }
        }

        items.push(item);
    }

    Some(FaultList { items })
}
