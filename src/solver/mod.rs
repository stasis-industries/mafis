pub mod astar;
pub mod heuristics;
pub mod lifelong;
pub mod pbs_planner;
pub mod pibt;
pub mod pibt_core;
pub mod pibt_window_planner;
pub mod priority_astar_planner;
pub mod rhcr;
pub mod token_common;
pub mod token_passing;
pub mod traits;
pub mod windowed;
pub mod guidance;
pub mod rt_lacam;
pub mod tpts;
pub mod apf_guidance;

use bevy::prelude::*;

use self::pibt::{PibtLifelongSolver, default_active_solver};
use self::rhcr::{RhcrConfig, RhcrMode, RhcrSolver};
use self::token_passing::TokenPassingSolver;
use self::rt_lacam::RtLaCAMSolver;
use self::tpts::TptsSolver;
use self::apf_guidance::PibtApfSolver;
use self::lifelong::LifelongSolver;

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
    ("pibt+apf", "PIBT+APF — Priority Inheritance with Potential Fields"),
];

/// Create a LifelongSolver by name with auto-computed defaults.
/// `grid_area` and `num_agents` are used for RHCR auto-config.
pub fn lifelong_solver_from_name(
    name: &str,
    grid_area: usize,
    num_agents: usize,
) -> Option<Box<dyn LifelongSolver>> {
    // PIBT+APF is a dedicated solver (not a GuidanceLayer wrapper) because
    // the paper requires APF to be updated sequentially inside the PIBT recursion.
    if name == "pibt+apf" {
        return Some(Box::new(PibtApfSolver::new()));
    }

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
    fn factory_creates_pibt_apf() {
        let solver = lifelong_solver_from_name("pibt+apf", 100, 10);
        assert!(solver.is_some());
        assert_eq!(solver.unwrap().name(), "pibt+apf");
    }

    #[test]
    fn factory_unknown_base_returns_none() {
        assert!(lifelong_solver_from_name("unknown+apf", 100, 10).is_none());
    }

    #[test]
    fn factory_unknown_layer_returns_none() {
        assert!(lifelong_solver_from_name("pibt+unknown", 100, 10).is_none());
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
    fn solver_names_has_eight_entries() {
        assert_eq!(SOLVER_NAMES.len(), 8);
    }
}
