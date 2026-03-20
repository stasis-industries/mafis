use bevy::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub enum ExportTrigger {
    Manual,
    Finished,
    Periodic(u64),
    FaultEvent(u64),
}

impl std::fmt::Display for ExportTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportTrigger::Manual => write!(f, "manual"),
            ExportTrigger::Finished => write!(f, "finished"),
            ExportTrigger::Periodic(t) => write!(f, "periodic_{t}"),
            ExportTrigger::FaultEvent(t) => write!(f, "fault_{t}"),
        }
    }
}

#[derive(Message)]
pub struct ExportRequest {
    pub trigger: ExportTrigger,
    pub json: bool,
    pub csv: bool,
}

#[derive(Resource, Debug, Clone)]
pub struct ExportConfig {
    pub periodic_enabled: bool,
    pub periodic_interval: u64,
    pub last_periodic_tick: Option<u64>,
    pub auto_on_finished: bool,
    pub auto_on_fault: bool,
    pub export_json: bool,
    pub export_csv: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            periodic_enabled: false,
            periodic_interval: 50,
            last_periodic_tick: None,
            auto_on_finished: false,
            auto_on_fault: false,
            export_json: true,
            export_csv: true,
        }
    }
}
