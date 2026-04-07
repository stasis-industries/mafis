pub mod lacam3;
pub mod lifelong;
pub mod pibt;
pub mod rhcr;
pub mod shared;
pub mod token;

// ---------------------------------------------------------------------------
// Backward-compatible re-exports — external callers keep working
// ---------------------------------------------------------------------------

// `crate::solver::heuristics::*` still works
pub use shared::heuristics;
// `crate::solver::traits::*` still works
pub use shared::traits;
// `crate::solver::pibt::PibtLifelongSolver` etc. still work (via pibt/mod.rs re-exports)

use bevy::prelude::*;

use self::lacam3::LaCAM3LifelongSolver;
use self::lifelong::LifelongSolver;
use self::pibt::{PibtLifelongSolver, default_active_solver};
use self::rhcr::{RhcrConfig, RhcrSolver};
use self::token::TokenPassingSolver;

// ---------------------------------------------------------------------------
// Solver registry
// ---------------------------------------------------------------------------

/// All available solver names with human-readable labels.
///
/// Fidelity discipline: every solver in this registry has a faithful Rust
/// implementation traceable to a public reference source under `docs/papers_codes/`.
pub const SOLVER_NAMES: &[(&str, &str)] = &[
    ("pibt", "PIBT — Priority Inheritance with Backtracking"),
    ("rhcr_pbs", "RHCR (PBS) — Rolling-Horizon with Priority-Based Search"),
    ("token_passing", "Token Passing — Decentralized Sequential Planning"),
    ("lacam3_lifelong", "LaCAM3 — Engineered LaCAM* (AAMAS 2024) Lifelong Wrapper"),
];

/// Create a LifelongSolver by name with auto-computed defaults.
/// `grid_area` and `num_agents` are used for RHCR auto-config.
pub fn lifelong_solver_from_name(
    name: &str,
    grid_area: usize,
    num_agents: usize,
) -> Option<Box<dyn LifelongSolver>> {
    match name {
        "pibt" => Some(Box::new(PibtLifelongSolver::new())),
        "rhcr_pbs" => {
            let cfg = RhcrConfig::auto(grid_area, num_agents);
            Some(Box::new(RhcrSolver::new(cfg)))
        }
        "token_passing" => Some(Box::new(TokenPassingSolver::new())),
        "lacam3_lifelong" => Some(Box::new(LaCAM3LifelongSolver::new())),
        _ => None,
    }
}

// Re-export for convenience
pub use self::lifelong::ActiveSolver;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct SolverPlugin;

impl Plugin for SolverPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(default_active_solver());
    }
}

// ---------------------------------------------------------------------------
// Factory integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod factory_tests {
    use super::*;

    #[test]
    fn factory_unknown_returns_none() {
        assert!(lifelong_solver_from_name("unknown", 100, 10).is_none());
    }

    #[test]
    fn factory_existing_solvers_still_work() {
        for &(name, _) in SOLVER_NAMES.iter() {
            assert!(
                lifelong_solver_from_name(name, 100, 10).is_some(),
                "factory should create '{name}'"
            );
        }
    }

    #[test]
    fn solver_names_has_four_entries() {
        assert_eq!(SOLVER_NAMES.len(), 4);
    }
}
