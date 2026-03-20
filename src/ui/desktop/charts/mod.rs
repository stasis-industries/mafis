pub mod throughput;
pub mod tasks;
pub mod heat;

// Re-export theme chart colors for use in chart modules
pub use super::theme::{CHART_PRIMARY, CHART_SECONDARY, CHART_BASELINE, CHART_HEAT};
