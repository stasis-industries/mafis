//! Built-in planners: one folder each, plus `toolkit/` for the planning
//! blocks they share. Adding a planner means one folder that exports an
//! `ENTRY`, and one line in `REGISTRY`.

pub mod pibt;
// RHCR is compute-intensive (PBS tree search with O(N × cascade) plan_agent
// calls per window) and requires rayon parallelism on the root-node build to
// stay interactive at realistic agent counts. WASM is single-threaded and
// cannot run rayon, so RHCR is **native-only**. The web build gets PIBT,
// Token Passing — both of which hit 60 FPS at 200+ agents in WASM.
// RHCR users should run the desktop build.
#[cfg(not(target_arch = "wasm32"))]
pub mod rhcr;
pub mod token_passing;
pub mod toolkit;

use crate::planning::registry::PlannerEntry;

/// Every built-in planner, in display order.
pub static REGISTRY: &[PlannerEntry] = &[
    pibt::ENTRY,
    #[cfg(not(target_arch = "wasm32"))]
    rhcr::ENTRY,
    token_passing::ENTRY,
];

/// The planner selected when nothing else is chosen, and the one shared tests run.
pub const DEFAULT: &str = pibt::ID;
