pub mod astar;
pub mod heuristics;
pub mod lifelong;
pub mod pbs_planner;
pub mod pibt;
pub mod pibt_core;
pub mod pibt_window_planner;
pub mod priority_astar_planner;
pub mod rhcr;
pub mod token_passing;
pub mod traits;
pub mod windowed;

use bevy::prelude::*;

use self::pibt::{PibtLifelongSolver, default_active_solver};
use self::rhcr::{RhcrConfig, RhcrMode, RhcrSolver};
use self::token_passing::TokenPassingSolver;
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
