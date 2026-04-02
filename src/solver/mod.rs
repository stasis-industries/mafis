pub mod shared;
pub mod lifelong;
pub mod pibt;
pub mod rhcr;
pub mod token;
pub mod rt_lacam;

// ---------------------------------------------------------------------------
// Backward-compatible re-exports — external callers keep working
// ---------------------------------------------------------------------------

// `crate::solver::heuristics::*` still works
pub use shared::heuristics;
// `crate::solver::traits::*` still works
pub use shared::traits;
// `crate::solver::pibt::PibtLifelongSolver` etc. still work (via pibt/mod.rs re-exports)

use bevy::prelude::*;

use self::lifelong::LifelongSolver;
use self::pibt::{PibtLifelongSolver, default_active_solver};
use self::rhcr::{RhcrConfig, RhcrMode, RhcrSolver};
use self::rt_lacam::RtLaCAMSolver;
use self::token::TokenPassingSolver;
use self::token::TptsSolver;

// ---------------------------------------------------------------------------
// Solver registry
// ---------------------------------------------------------------------------

/// All available solver names with human-readable labels.
pub const SOLVER_NAMES: &[(&str, &str)] = &[
    ("pibt", "PIBT — Priority Inheritance with Backtracking"),
    ("rhcr_pbs", "RHCR (PBS) — Rolling-Horizon with Priority-Based Search"),
    ("rhcr_pibt", "RHCR (PIBT-Window) — Rolling-Horizon with PIBT"),
    ("rhcr_priority_astar", "RHCR (Priority A*) — Rolling-Horizon with Priority A*"),
    ("token_passing", "Token Passing — Decentralized Sequential Planning"),
    ("rt_lacam", "RT-LaCAM — Real-Time Configuration-Space Search"),
    ("tpts", "TPTS — Token Passing with Task Swaps"),
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
            let cfg = RhcrConfig::auto(RhcrMode::Pbs, grid_area, num_agents);
            Some(Box::new(RhcrSolver::new(cfg)))
        }
        "rhcr_pibt" => {
            let cfg = RhcrConfig::auto(RhcrMode::PibtWindow, grid_area, num_agents);
            Some(Box::new(RhcrSolver::new(cfg)))
        }
        "rhcr_priority_astar" => {
            let cfg = RhcrConfig::auto(RhcrMode::PriorityAStar, grid_area, num_agents);
            Some(Box::new(RhcrSolver::new(cfg)))
        }
        "token_passing" => Some(Box::new(TokenPassingSolver::new())),
        "rt_lacam" => Some(Box::new(RtLaCAMSolver::new(grid_area, num_agents))),
        "tpts" => Some(Box::new(TptsSolver::new())),
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
    fn factory_creates_rt_lacam() {
        let solver = lifelong_solver_from_name("rt_lacam", 100, 10);
        assert!(solver.is_some());
        assert_eq!(solver.unwrap().name(), "rt_lacam");
    }

    #[test]
    fn factory_creates_tpts() {
        let solver = lifelong_solver_from_name("tpts", 100, 10);
        assert!(solver.is_some());
        assert_eq!(solver.unwrap().name(), "tpts");
    }

    #[test]
    fn factory_unknown_returns_none() {
        assert!(lifelong_solver_from_name("unknown", 100, 10).is_none());
    }

    #[test]
    fn factory_existing_solvers_still_work() {
        for &(name, _) in SOLVER_NAMES.iter().filter(|(n, _)| !n.contains('+')) {
            assert!(
                lifelong_solver_from_name(name, 100, 10).is_some(),
                "factory should create '{name}'"
            );
        }
    }

    #[test]
    fn solver_names_has_seven_entries() {
        assert_eq!(SOLVER_NAMES.len(), 7);
    }
}
