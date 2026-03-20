use bevy::prelude::*;
use std::collections::HashMap;

/// Persistent desktop UI state — panel visibility, section collapse, etc.
#[derive(Resource)]
pub struct DesktopUiState {
    pub show_left_panel: bool,
    pub show_right_panel: bool,
    pub show_toolbar: bool,
    pub show_timeline: bool,

    /// Collapsible section open/closed state, keyed by section ID.
    pub sections: HashMap<&'static str, bool>,

    // Desktop-exclusive panels
    pub show_profiling: bool,
    pub show_experiment: bool,

    /// Full-page experiment mode — hides all other panels.
    pub experiment_fullpage: bool,

    /// Manual fault injection coordinates (persisted across frames).
    pub manual_fault_x: i32,
    pub manual_fault_y: i32,
}

impl Default for DesktopUiState {
    fn default() -> Self {
        let mut sections = HashMap::new();
        sections.insert("simulation", true);
        sections.insert("solver", true);
        sections.insert("topology", true);
        sections.insert("fault", false);
        sections.insert("visualization", false);
        sections.insert("export", false);
        sections.insert("status", true);
        sections.insert("scorecard", true);
        sections.insert("performance", true);
        sections.insert("fault_response", true);
        sections.insert("agents", false);

        Self {
            show_left_panel: true,
            show_right_panel: true,
            show_toolbar: true,
            show_timeline: true,
            sections,
            show_profiling: false,
            show_experiment: false,
            experiment_fullpage: false,
            manual_fault_x: 0,
            manual_fault_y: 0,
        }
    }
}
