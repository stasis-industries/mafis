//! Headless simulation tests.
//!
//! These live in `src/` (not `tests/`) so that the library is compiled with
//! `cfg(test)` set. This activates the `#[cfg(not(any(test, feature = "headless")))]`
//! guards in `AnalysisPlugin` and `FaultPlugin`, excluding render-dependent
//! systems that require `Assets<Mesh/Image/StandardMaterial>` which are
//! unavailable in headless MinimalPlugins test builds.
//!
//! # Adding tests
//! Add new test modules here and reference them below.

pub mod common;
pub mod simulation;
