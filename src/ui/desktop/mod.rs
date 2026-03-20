pub mod charts;
pub mod panels;
pub mod persistence;
pub mod shortcuts;
pub mod state;
pub mod theme;
pub mod timeline;
pub mod toolbar;

use bevy::prelude::*;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass};

use self::panels::experiment::{ExperimentCommand, ExperimentGuiState, ExperimentHandle};
use self::state::DesktopUiState;

use crate::core::state::SimulationConfig;
use crate::core::topology::ActiveTopology;
use crate::core::task::ActiveScheduler;
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
        // Load persisted settings and apply to UiState
        let settings = persistence::PersistedSettings::load();

        app.add_plugins(EguiPlugin::default())
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_resource::<DesktopUiState>()
            .init_resource::<ThemeApplied>()
            .init_resource::<ExperimentGuiState>()
            .insert_resource(PersistedSettingsRes(settings))
            .add_systems(
                EguiPrimaryContextPass,
                (
                    theme::apply_theme_once
                        .run_if(|applied: Res<ThemeApplied>| !applied.0),
                    // Toolbar and timeline (TopBottomPanels) must run before
                    // SidePanels so they span the full window width.
                    toolbar::toolbar_ui,
                    timeline::timeline_ui
                        .after(toolbar::toolbar_ui)
                        .run_if(|d: Res<DesktopUiState>| !d.experiment_fullpage && d.show_timeline),
                    panels::left_panel_ui
                        .after(timeline::timeline_ui)
                        .run_if(|d: Res<DesktopUiState>| !d.experiment_fullpage),
                    panels::right_panel_ui
                        .after(timeline::timeline_ui)
                        .run_if(|d: Res<DesktopUiState>| !d.experiment_fullpage),
                    experiment_fullpage_ui
                        .after(toolbar::toolbar_ui),
                ),
            )
            .add_systems(
                Update,
                (
                    shortcuts::handle_shortcuts.in_set(BridgeSet),
                    process_experiment_commands,
                ),
            );
    }
}

/// Wrapper to store persisted settings as a Bevy resource.
#[derive(Resource)]
pub struct PersistedSettingsRes(pub persistence::PersistedSettings);

/// Full-page experiment view — CentralPanel that takes over the viewport.
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
                solver, topology: topo, scheduler: sched,
                num_agents, seed, tick_count,
            } => {
                // Exit experiment mode
                desktop.experiment_fullpage = false;
                // Pre-configure the simulator
                ui_state.solver_name = solver;
                ui_state.topology_name = topo.clone();
                ui_state.num_agents = num_agents;
                ui_state.seed = seed;
                config.duration = tick_count;
                // Apply topology: check registry first, fall back to built-in
                if let Some(entry) = topo_registry.entries.iter().find(|e| e.id == topo) {
                    if let Some((grid, zones)) = crate::core::topology::TopologyRegistry::parse_entry(entry) {
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
    // Check if a running experiment has completed
    if let Some(ref h) = handle {
        if h.done.load(std::sync::atomic::Ordering::Acquire) {
            let mut result = h.result.lock().unwrap();
            if let Some(res) = result.take() {
                gui.last_result = Some(res);
                commands.remove_resource::<ExperimentHandle>();
            }
        }
    }
}
