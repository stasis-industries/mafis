pub mod constants;
pub mod pbs_planner;
pub mod planner;
pub mod windowed;
pub use planner::{PARAMS, RhcrConfig, RhcrPlanner};
pub use windowed::{PlanFragment, WindowAgent, WindowContext, WindowResult, WindowedPlanner};

use crate::core::topology::ZoneMap;
use crate::planning::params::ResolvedParams;
use crate::planning::planner::Planner;
use crate::planning::registry::{PlannerEntry, PlannerInfo};

pub const ID: &str = "rhcr_pbs";

/// Largest fleet RHCR-PBS supports; experiments leave larger fleets out.
const FLEET_LIMIT: usize = 200;

pub const ENTRY: PlannerEntry = PlannerEntry {
    info: PlannerInfo {
        id: ID,
        label: "RHCR (PBS)",
        summary: "Windowed planning: Priority-Based Search over the next H ticks, replanning every W ticks.",
        reference: "Li et al., AAAI 2021",
        params: PARAMS,
        assigns_tasks: false,
    },
    build,
    fleet_limit,
    type_name: std::any::type_name::<RhcrPlanner>,
};

fn build(params: &ResolvedParams, fleet: usize) -> Box<dyn Planner> {
    Box::new(RhcrPlanner::new(RhcrConfig::from_params(params, fleet)))
}

fn fleet_limit(_zones: &ZoneMap) -> Option<usize> {
    Some(FLEET_LIMIT)
}
