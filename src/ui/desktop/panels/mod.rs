pub mod agent_list;
pub mod experiment;
pub mod export;
pub mod fault;
pub mod fault_response;
pub mod performance;
pub mod profiling;
pub mod scorecard;
pub mod simulation;
pub mod solver;
pub mod status;
pub mod visualization;

use bevy::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy_egui::EguiContexts;

use bevy::diagnostic::DiagnosticsStore;

use crate::analysis::baseline::{BaselineDiff, BaselineStore};
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::heatmap::HeatmapState;
use crate::analysis::scorecard::ResilienceScorecard;
use crate::analysis::AnalysisConfig;
use crate::core::grid::GridMap;
use crate::core::live_sim::LiveSim;
use crate::core::state::{SimState, SimulationConfig};
use crate::core::task::ActiveScheduler;
use crate::core::topology::{ActiveTopology, TopologyRegistry};
use crate::export::config::ExportConfig;
use crate::fault::manual::ManualFaultCommand;
use crate::render::animator::RobotOpacity;
use crate::render::graphics::GraphicsConfig;
use crate::render::orbit_camera::OrbitCamera;
use crate::ui::controls::UiState;

/// Bundled visualization resources to stay under Bevy's 16-param limit.
#[derive(SystemParam)]
pub struct VisResources<'w> {
    analysis_config: ResMut<'w, AnalysisConfig>,
    heatmap: ResMut<'w, HeatmapState>,
    graphics: ResMut<'w, GraphicsConfig>,
    orbit: ResMut<'w, OrbitCamera>,
    grid: Res<'w, GridMap>,
    robot_opacity: ResMut<'w, RobotOpacity>,
}

use super::charts;
use super::state::DesktopUiState;

use self::experiment::{ExperimentGuiState, ExperimentHandle};

pub fn left_panel_ui(
    mut contexts: EguiContexts,
    mut desktop: ResMut<DesktopUiState>,
    sim_state: Res<State<SimState>>,
    mut ui_state: ResMut<UiState>,
    mut config: ResMut<SimulationConfig>,
    mut scheduler: ResMut<ActiveScheduler>,
    mut topology: ResMut<ActiveTopology>,
    mut vis: VisResources,
    mut export_config: ResMut<ExportConfig>,
    topo_registry: Res<TopologyRegistry>,
    mut manual_cmds: MessageWriter<ManualFaultCommand>,
    mut export_requests: MessageWriter<crate::export::config::ExportRequest>,
) -> Result {
    if !desktop.show_left_panel {
        return Ok(());
    }

    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    let state = **sim_state;

    egui::SidePanel::left("left_panel")
        .default_width(340.0)
        .min_width(280.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::CollapsingHeader::new("Simulation")
                    .default_open(true)
                    .show(ui, |ui| {
                        simulation::simulation_panel(
                            ui,
                            &mut ui_state,
                            &mut config,
                            state,
                            &mut scheduler,
                            &mut topology,
                            &topo_registry,
                        );
                    });

                ui.separator();

                egui::CollapsingHeader::new("Solver & Scheduling")
                    .default_open(true)
                    .show(ui, |ui| {
                        solver::solver_panel(ui, &mut ui_state, state);
                    });

                ui.separator();

                egui::CollapsingHeader::new("Fault Injection")
                    .default_open(false)
                    .show(ui, |ui| {
                        let d = &mut *desktop;
                        let output = fault::fault_panel(
                            ui, &mut ui_state, state,
                            &mut d.manual_fault_x,
                            &mut d.manual_fault_y,
                        );
                        for cmd in output.manual_cmds {
                            manual_cmds.write(cmd);
                        }
                    });

                ui.separator();

                egui::CollapsingHeader::new("Visualization")
                    .default_open(false)
                    .show(ui, |ui| {
                        visualization::visualization_panel(
                            ui,
                            &mut vis.analysis_config,
                            &mut vis.heatmap,
                            &mut vis.graphics,
                            &mut vis.orbit,
                            &vis.grid,
                            &mut vis.robot_opacity,
                        );
                    });

                ui.separator();

                egui::CollapsingHeader::new("Data Export")
                    .default_open(false)
                    .show(ui, |ui| {
                        let is_running = state == SimState::Running
                            || state == SimState::Paused
                            || state == SimState::Finished;
                        let output = export::export_panel(ui, &mut export_config, is_running);
                        if let Some(req) = output.export_request {
                            export_requests.write(req);
                        }
                    });
            });
        });

    Ok(())
}

pub fn right_panel_ui(
    mut contexts: EguiContexts,
    desktop: Res<DesktopUiState>,
    sim_state: Res<State<SimState>>,
    config: Res<SimulationConfig>,
    live_sim: Option<Res<LiveSim>>,
    scorecard: Res<ResilienceScorecard>,
    fault_metrics: Res<FaultMetrics>,
    baseline_store: Res<BaselineStore>,
    baseline_diff: Res<BaselineDiff>,
    diagnostics: Res<DiagnosticsStore>,
    _experiment_gui: ResMut<ExperimentGuiState>,
    _experiment_handle: Option<Res<ExperimentHandle>>,
    mut manual_cmds: MessageWriter<ManualFaultCommand>,
) -> Result {
    if !desktop.show_right_panel {
        return Ok(());
    }

    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    egui::SidePanel::right("right_panel")
        .default_width(320.0)
        .min_width(260.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── Status ─────────────────────────────────
                egui::CollapsingHeader::new("Status")
                    .default_open(true)
                    .show(ui, |ui| {
                        status::status_panel(
                            ui,
                            &sim_state,
                            &config,
                            live_sim.as_deref(),
                        );
                    });

                ui.separator();

                // ── Scorecard ──────────────────────────────
                if scorecard.has_faults {
                    egui::CollapsingHeader::new("Scorecard")
                        .default_open(true)
                        .show(ui, |ui| {
                            scorecard::scorecard_panel(ui, &scorecard);
                        });
                    ui.separator();
                }

                // ── Performance ────────────────────────────
                if live_sim.is_some() {
                    egui::CollapsingHeader::new("Performance")
                        .default_open(true)
                        .show(ui, |ui| {
                            performance::performance_panel(
                                ui,
                                live_sim.as_deref(),
                                &baseline_store,
                                &baseline_diff,
                            );
                        });
                    ui.separator();
                }

                // ── Charts ─────────────────────────────────
                if let Some(ref sim) = live_sim {
                    egui::CollapsingHeader::new("Charts")
                        .default_open(true)
                        .show(ui, |ui| {
                            let bl = baseline_store.record.as_ref();

                            ui.label("Throughput");
                            charts::throughput::throughput_chart(
                                ui,
                                &sim.analysis,
                                bl,
                            );

                            ui.add_space(6.0);
                            ui.label("Tasks");
                            charts::tasks::tasks_chart(
                                ui,
                                &sim.analysis,
                                bl,
                            );

                            if !sim.analysis.heat_series.is_empty() {
                                ui.add_space(6.0);
                                ui.label("Heat");
                                charts::heat::heat_chart(ui, &sim.analysis);
                            }
                        });
                    ui.separator();
                }

                // ── Fault Response ─────────────────────────
                if !fault_metrics.event_records.is_empty() {
                    egui::CollapsingHeader::new("Fault Response")
                        .default_open(true)
                        .show(ui, |ui| {
                            fault_response::fault_response_panel(ui, &fault_metrics);
                        });
                    ui.separator();
                }

                // ── Agents ─────────────────────────────────
                egui::CollapsingHeader::new("Agents")
                    .default_open(false)
                    .show(ui, |ui| {
                        let output = agent_list::agent_list_panel(ui, live_sim.as_deref());
                        for cmd in output.manual_cmds {
                            manual_cmds.write(cmd);
                        }
                    });

                ui.separator();

                // ── Profiling ──────────────────────────────
                egui::CollapsingHeader::new("Profiling")
                    .default_open(false)
                    .show(ui, |ui| {
                        profiling::profiling_panel(ui, &diagnostics);
                    });
            });
        });

    Ok(())
}
