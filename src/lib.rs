#![allow(clippy::too_many_arguments, clippy::type_complexity)]

pub mod analysis;
pub mod constants;
pub mod core;
pub mod experiment;
pub mod export;
pub mod fault;
pub mod render;
pub mod solver;
pub mod ui;

#[cfg(feature = "mapf-pilot")]
pub mod pilot_bridge;

// Headless ECS integration tests. Lives in `src/` (not `tests/`) so that the
// library is compiled with `cfg(test)` set — required for `#[cfg(not(test))]`
// guards in AnalysisPlugin and FaultPlugin to exclude render-dependent systems.
#[cfg(test)]
mod sim_tests;

use bevy::prelude::*;

pub struct MapfFisPlugin;

impl Plugin for MapfFisPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            core::CorePlugin,
            solver::SolverPlugin,
            fault::FaultPlugin,
            analysis::AnalysisPlugin,
            export::ExportPlugin,
            render::RenderPlugin,
            ui::UiPlugin,
        ));

        #[cfg(feature = "mapf-pilot")]
        app.add_plugins(pilot_bridge::PilotBridgePlugin);
    }
}

