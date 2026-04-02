pub mod heat;
pub mod tasks;
pub mod throughput;

// Re-export theme chart colors for use in chart modules
pub use super::theme::{CHART_BASELINE, CHART_HEAT, CHART_PRIMARY, CHART_SECONDARY};
