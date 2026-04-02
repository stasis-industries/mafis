#[cfg(not(feature = "headless"))]
pub mod charts;
pub mod panels;
#[cfg(not(feature = "headless"))]
pub mod persistence;
#[cfg(not(feature = "headless"))]
pub mod shortcuts;
pub mod state;
pub mod theme;
#[cfg(not(feature = "headless"))]
pub mod timeline;
#[cfg(not(feature = "headless"))]
pub mod toolbar;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};

use self::panels::experiment::{ExperimentCommand, ExperimentGuiState, ExperimentHandle};
use self::state::DesktopUiState;

#[cfg(not(feature = "headless"))]
use crate::core::state::SimulationConfig;
#[cfg(not(feature = "headless"))]
use crate::core::task::ActiveScheduler;
#[cfg(not(feature = "headless"))]
use crate::core::topology::ActiveTopology;
#[cfg(not(feature = "headless"))]
use crate::ui::controls::UiState;

/// Stub SystemSet for native builds — replaces BridgeSet from bridge.rs.
/// Desktop command processing runs in this set so FaultPlugin's
/// `.after(BridgeSet)` ordering still works.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeSet;

/// Tracks whether the egui theme has been applied. Exposed for tests.
#[derive(Resource, Default)]
pub struct ThemeApplied(pub bool);

pub struct DesktopUiPlugin;

impl Plugin for DesktopUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<ThemeApplied>()
            .init_resource::<ExperimentGuiState>();

        // ── Headless (experiment-only): force fullpage, no observatory ────
        #[cfg(feature = "headless")]
        {
            let mut state = DesktopUiState::default();
            state.experiment_fullpage = true;
            app.insert_resource(state);

            // Spawn a 2D camera so bevy_egui has a render target
            app.add_systems(Startup, |mut commands: Commands| {
                commands.spawn(Camera2d);
            });

            app.add_systems(
                EguiPrimaryContextPass,
                (
                    theme::apply_theme_once.run_if(|applied: Res<ThemeApplied>| !applied.0),
                    experiment_fullpage_ui,
                ),
            )
            .add_systems(Update, process_experiment_commands);
        }

        // ── Full desktop (observatory + experiments) ─────────────────────
        #[cfg(not(feature = "headless"))]
        {
            use bevy::diagnostic::FrameTimeDiagnosticsPlugin;

            let settings = persistence::PersistedSettings::load();

            app.add_plugins(FrameTimeDiagnosticsPlugin::default())
                .init_resource::<DesktopUiState>()
                .insert_resource(PersistedSettingsRes(settings))
                .add_systems(
                    EguiPrimaryContextPass,
                    (
                        theme::apply_theme_once.run_if(|applied: Res<ThemeApplied>| !applied.0),
                        toolbar::toolbar_ui,
                        timeline::timeline_ui.after(toolbar::toolbar_ui).run_if(
                            |d: Res<DesktopUiState>| !d.experiment_fullpage && d.show_timeline,
                        ),
                        panels::left_panel_ui
                            .after(timeline::timeline_ui)
                            .run_if(|d: Res<DesktopUiState>| !d.experiment_fullpage),
                        panels::right_panel_ui
                            .after(timeline::timeline_ui)
                            .run_if(|d: Res<DesktopUiState>| !d.experiment_fullpage),
                        experiment_fullpage_ui.after(toolbar::toolbar_ui),
                    ),
                )
                .add_systems(
                    Update,
                    (shortcuts::handle_shortcuts.in_set(BridgeSet), process_experiment_commands),
                );
        }
    }
}

/// Wrapper to store persisted settings as a Bevy resource.
#[cfg(not(feature = "headless"))]
#[derive(Resource)]
pub struct PersistedSettingsRes(pub persistence::PersistedSettings);

/// Full-page experiment view — CentralPanel that takes over the viewport.
///
/// In headless mode this is the only UI. In full desktop mode it activates
/// when the user clicks "EXPERIMENTS" in the toolbar.
#[cfg(feature = "headless")]
fn experiment_fullpage_ui(
    mut contexts: EguiContexts,
    mut gui: ResMut<ExperimentGuiState>,
    handle: Option<Res<ExperimentHandle>>,
    mut commands: Commands,
    topo_registry: Res<crate::core::topology::TopologyRegistry>,
) -> Result {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    let mut exp_cmds = Vec::new();

    egui::CentralPanel::default().show(ctx, |ui| {
        panels::experiment::experiment_fullpage_panel(
            ui,
            &mut gui,
            handle.as_deref(),
            &mut exp_cmds,
            &topo_registry,
        );
    });

    for cmd in exp_cmds {
        match cmd {
            ExperimentCommand::Launch(matrix) => {
                let exp_handle = panels::experiment::launch_experiment(matrix);
                commands.insert_resource(exp_handle);
            }
            ExperimentCommand::ClearHandle => {
                commands.remove_resource::<ExperimentHandle>();
            }
        }
    }

    Ok(())
}

/// Full-page experiment view — observatory mode (includes SimulateIn3D).
#[cfg(not(feature = "headless"))]
fn experiment_fullpage_ui(
    mut contexts: EguiContexts,
    mut gui: ResMut<ExperimentGuiState>,
    handle: Option<Res<ExperimentHandle>>,
    mut commands: Commands,
    mut desktop: ResMut<DesktopUiState>,
    mut ui_state: ResMut<UiState>,
    mut config: ResMut<SimulationConfig>,
    mut scheduler: ResMut<ActiveScheduler>,
    mut topology: ResMut<ActiveTopology>,
    topo_registry: Res<crate::core::topology::TopologyRegistry>,
) -> Result {
    if !desktop.experiment_fullpage {
        return Ok(());
    }

    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    let mut exp_cmds = Vec::new();

    egui::CentralPanel::default().show(ctx, |ui| {
        panels::experiment::experiment_fullpage_panel(
            ui,
            &mut gui,
            handle.as_deref(),
            &mut exp_cmds,
            &topo_registry,
        );
    });

    for cmd in exp_cmds {
        match cmd {
            ExperimentCommand::Launch(matrix) => {
                let exp_handle = panels::experiment::launch_experiment(matrix);
                commands.insert_resource(exp_handle);
            }
            ExperimentCommand::ClearHandle => {
                commands.remove_resource::<ExperimentHandle>();
            }
            ExperimentCommand::SimulateIn3D {
                solver,
                topology: topo,
                scheduler: sched,
                num_agents,
                seed,
                tick_count,
            } => {
                desktop.experiment_fullpage = false;
                ui_state.solver_name = solver;
                ui_state.topology_name = topo.clone();
                ui_state.num_agents = num_agents;
                ui_state.seed = seed;
                config.duration = tick_count;
                if let Some(entry) = topo_registry.entries.iter().find(|e| e.id == topo) {
                    if let Some((grid, zones)) =
                        crate::core::topology::TopologyRegistry::parse_entry(entry)
                    {
                        topology.set(Box::new(crate::core::topology::CustomMap { grid, zones }));
                    }
                } else {
                    *topology = ActiveTopology::from_name(&topo);
                }
                *scheduler = ActiveScheduler::from_name(&sched);
            }
        }
    }

    Ok(())
}

/// Processes experiment commands generated by the experiment panel.
/// Runs in Update because it needs Commands to insert/remove resources.
fn process_experiment_commands(
    mut commands: Commands,
    mut gui: ResMut<ExperimentGuiState>,
    handle: Option<Res<ExperimentHandle>>,
) {
    if let Some(ref h) = handle {
        if h.done.load(std::sync::atomic::Ordering::Acquire) {
            let mut result = h.result.lock().unwrap();
            if let Some(res) = result.take() {
                // Auto-save results to results/ directory
                auto_save_results(&res);
                gui.last_result = Some(res);
                commands.remove_resource::<ExperimentHandle>();
            }
        }
    }
}

/// Automatically save experiment results to results/ as CSV files.
fn auto_save_results(result: &crate::experiment::runner::MatrixResult) {
    use crate::experiment::export::{write_runs_csv, write_summary_csv};
    use std::fs;

    if let Err(e) = fs::create_dir_all("results") {
        eprintln!("Failed to create results/: {e}");
        return;
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let runs_path = format!("results/experiment_{timestamp}_runs.csv");
    match fs::File::create(&runs_path) {
        Ok(mut f) => {
            if let Err(e) = write_runs_csv(&mut f, &result.runs) {
                eprintln!("Failed to write {runs_path}: {e}");
            } else {
                eprintln!("Auto-saved: {runs_path} ({} rows)", result.runs.len() * 2);
            }
        }
        Err(e) => eprintln!("Failed to create {runs_path}: {e}"),
    }

    let summary_path = format!("results/experiment_{timestamp}_summary.csv");
    match fs::File::create(&summary_path) {
        Ok(mut f) => {
            if let Err(e) = write_summary_csv(&mut f, &result.summaries) {
                eprintln!("Failed to write {summary_path}: {e}");
            } else {
                eprintln!("Auto-saved: {summary_path} ({} rows)", result.summaries.len());
            }
        }
        Err(e) => eprintln!("Failed to create {summary_path}: {e}"),
    }
}
