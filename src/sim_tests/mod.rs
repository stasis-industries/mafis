//! Headless simulation tests.
//!
//! These live in `src/` (not `tests/`) so that the library is compiled with
//! `cfg(test)` set. This allows the `#[cfg(not(test))]` guards in
//! `analysis::AnalysisPlugin` and `fault::FaultPlugin` to exclude
//! render-dependent systems (e.g. `setup_heatmap_palette`, `process_manual_faults`)
//! that require `Assets<Mesh/Image/StandardMaterial>` which are unavailable in
//! headless MinimalPlugins test builds.
//!
//! # Adding tests
//! Add new test modules here and reference them below.

pub mod common;
pub mod simulation;
