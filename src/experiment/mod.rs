//! Headless experiment pipeline — runs matrix of configs, produces paired
//! baseline/faulted results with differential metrics and statistical summaries.
//!
//! Available on both native and WASM. Native uses rayon for parallel execution;
//! WASM exposes single-run + finish API for async JS-driven loops.

pub mod config;
pub mod export;
pub mod metrics;
pub mod runner;
pub mod stats;
#[cfg(not(target_arch = "wasm32"))]
pub mod suite;

// Re-export key types for convenience.
pub use config::{ExperimentConfig, ExperimentMatrix};
#[cfg(not(target_arch = "wasm32"))]
pub use runner::{ExperimentProgress, run_matrix};
pub use runner::{MatrixResult, RunResult, run_single_experiment};
