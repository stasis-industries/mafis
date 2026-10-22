pub mod heuristics;
pub mod params;
pub mod planner;
pub mod traits;

pub use heuristics::{
    DistanceMap, DistanceMapCache, compute_distance_maps, delta_to_action, manhattan,
};
pub use planner::ActiveSolver;
pub use traits::{MAPFSolver, Optimality, Scalability, SolverError, SolverInfo};

use bevy::prelude::*;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct SolverPlugin;

impl Plugin for SolverPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(crate::planners::pibt::default_active_solver());
    }
}
