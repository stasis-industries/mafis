#[cfg(target_arch = "wasm32")]
pub mod bridge;
pub mod controls;
#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;

use bevy::prelude::*;

// Re-export BridgeSet from whichever module provides it.
// FaultPlugin references `crate::ui::BridgeSet` for system ordering.
#[cfg(target_arch = "wasm32")]
pub use bridge::BridgeSet;
#[cfg(not(target_arch = "wasm32"))]
pub use desktop::BridgeSet;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        // ControlsPlugin drives live simulation (Loading → Running → Finished)
        // and imports render types — observatory only.
        #[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
        app.add_plugins(controls::ControlsPlugin);

        // In headless mode, still init UiState resource (queried by other systems)
        #[cfg(feature = "headless")]
        app.init_resource::<controls::UiState>();

        #[cfg(target_arch = "wasm32")]
        app.add_plugins(bridge::BridgePlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(desktop::DesktopUiPlugin);
    }
}
