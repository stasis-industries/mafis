pub mod analysis;
pub mod constants;
pub mod core;
pub mod experiment;
pub mod export;
pub mod fault;
pub mod planners;
pub mod planning;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
pub mod render;
pub mod ui;

// Headless ECS integration tests. Lives in `src/` (not `tests/`) so that the
// library is compiled with `cfg(test)` set — required for
// `#[cfg(not(any(test, feature = "headless")))]` guards in AnalysisPlugin and
// FaultPlugin to exclude render-dependent systems.
#[cfg(test)]
mod testing;

use bevy::prelude::*;

pub struct MapfFisPlugin;

impl Plugin for MapfFisPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            core::CorePlugin,
            planning::SolverPlugin,
            fault::FaultPlugin,
            analysis::AnalysisPlugin,
            ui::UiPlugin,
        ));

        // Render + Export only in observatory mode (WASM or non-headless desktop)
        #[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
        app.add_plugins((render::RenderPlugin, export::ExportPlugin));
    }
}
