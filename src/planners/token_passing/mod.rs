pub(crate) mod common;
pub mod planner;
pub mod task_selection;
#[cfg(test)]
mod tests;
pub use planner::TokenPassingPlanner;
pub use task_selection::TokenPassingTaskSelection;

use crate::core::topology::ZoneMap;
use crate::planning::params::ResolvedParams;
use crate::planning::planner::Planner;
use crate::planning::registry::{PlannerEntry, PlannerInfo};

pub const ID: &str = "token_passing";

pub const ENTRY: PlannerEntry = PlannerEntry {
    info: PlannerInfo {
        id: ID,
        label: "Token Passing",
        summary: "Decentralized: each idle agent plans against the paths others hold in a shared token.",
        reference: "Ma et al., AAMAS 2017",
        params: &[],
        assigns_tasks: true,
    },
    build,
    fleet_limit,
    type_name: std::any::type_name::<TokenPassingPlanner>,
};

fn build(_params: &ResolvedParams, _fleet: usize) -> Box<dyn Planner> {
    Box::new(TokenPassingPlanner::new())
}

/// One parking endpoint per agent (Ma et al. 2017, well-formedness condition 2).
fn fleet_limit(zones: &ZoneMap) -> Option<usize> {
    Some(zones.parking_cells().len())
}
