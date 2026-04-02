mod commands;
mod serialize;
#[cfg(target_arch = "wasm32")]
mod wasm_api;

use bevy::prelude::*;

// Re-export wasm_bindgen exports so they remain discoverable by wasm-bindgen.
#[cfg(target_arch = "wasm32")]
pub use wasm_api::*;

// ---------------------------------------------------------------------------
// Thread-local bridge state (Bevy <-> JS)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub(super) struct BridgeInner {
    pub(super) outgoing: Option<String>,
    pub(super) incoming: Vec<commands::JsCommand>,
}

#[cfg(target_arch = "wasm32")]
impl Default for BridgeInner {
    fn default() -> Self {
        Self { outgoing: None, incoming: Vec::new() }
    }
}

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static BRIDGE: RefCell<BridgeInner> = RefCell::new(BridgeInner::default());
}

// Used by wasm_api::send_command via super::parse_command
#[cfg(target_arch = "wasm32")]
use commands::parse_command;

// ---------------------------------------------------------------------------
// FPS tracker
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub(super) struct FpsTracker {
    pub(super) smoothed: f32,
}

impl Default for FpsTracker {
    fn default() -> Self {
        Self { smoothed: 60.0 }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// SystemSet for bridge command processing -- other systems that read
/// Messages written by the bridge should run `.after(BridgeSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeSet;

pub struct BridgePlugin;

impl Plugin for BridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsTracker>().add_systems(
            Update,
            (
                serialize::sync_state_to_js,
                commands::process_js_commands.in_set(BridgeSet),
            ),
        );
    }
}
