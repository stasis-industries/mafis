#[cfg(target_arch = "wasm32")]
pub mod bridge;
#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;
pub mod controls;

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
        app.add_plugins(controls::ControlsPlugin);

        #[cfg(target_arch = "wasm32")]
        app.add_plugins(bridge::BridgePlugin);

        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(desktop::DesktopUiPlugin);
    }
}
