pub mod planner;
pub use planner::PibtPlanner;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
pub(crate) use planner::PibtSolver;

use crate::planning::params::ResolvedParams;
use crate::planning::planner::Planner;
use crate::planning::registry::{PlannerEntry, PlannerInfo, no_fleet_limit};

pub const ID: &str = "pibt";

pub const ENTRY: PlannerEntry = PlannerEntry {
    info: PlannerInfo {
        id: ID,
        label: "PIBT",
        summary: "Reactive priority inheritance: plans one step for every agent, every tick.",
        reference: "Okumura et al., AIJ 2022",
        params: &[],
        assigns_tasks: false,
    },
    build,
    fleet_limit: no_fleet_limit,
    type_name: std::any::type_name::<PibtPlanner>,
};

fn build(_params: &ResolvedParams, _fleet: usize) -> Box<dyn Planner> {
    Box::new(PibtPlanner::new())
}
