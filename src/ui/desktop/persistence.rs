use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persistent settings saved to disk between sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSettings {
    // Last used simulation config
    pub last_topology: String,
    pub last_solver: String,
    pub last_scheduler: String,
    pub last_agent_count: usize,
    pub last_seed: u64,
    pub last_duration: u64,

    // Desktop-specific
    pub export_directory: Option<String>,

    // Panel visibility
    pub show_left_panel: bool,
    pub show_right_panel: bool,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            last_topology: "warehouse_large".to_string(),
            last_solver: "pibt".to_string(),
            last_scheduler: "random".to_string(),
            last_agent_count: 15,
            last_seed: 42,
            last_duration: 500,
            export_directory: None,
            show_left_panel: true,
            show_right_panel: true,
        }
    }
}

impl PersistedSettings {
    fn config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "MAFIS")
            .map(|dirs| dirs.config_dir().join("settings.ron"))
    }

    pub fn load() -> Self {
        Self::config_path()
            .and_then(|path| std::fs::read_to_string(&path).ok())
            .and_then(|content| ron::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Ok(content) = ron::to_string(self) {
                std::fs::write(&path, content).ok();
            }
        }
    }
}
