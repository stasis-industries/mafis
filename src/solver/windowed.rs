//! WindowedPlanner — inner trait for RHCR conflict resolution modes.
//!
//! Each mode (PBS, PIBT-Window, Priority A*) implements this trait.
//! The RHCR solver handles windowing, fallback, and buffer management.

use bevy::prelude::*;
use smallvec::SmallVec;

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::heuristics::DistanceMap;

// ---------------------------------------------------------------------------
// Context for windowed planning
// ---------------------------------------------------------------------------

pub struct WindowContext<'a> {
    pub grid: &'a GridMap,
    pub horizon: usize,
    pub node_limit: usize,
    /// Agent positions and goals for this replan window.
    pub agents: &'a [WindowAgent],
    /// Pre-computed distance maps aligned with `agents`.
    pub distance_maps: &'a [&'a DistanceMap],
}

#[derive(Clone, Debug)]
pub struct WindowAgent {
    pub index: usize,
    pub pos: IVec2,
    pub goal: IVec2,
    /// Additional goals after `goal` is reached within the planning window.
    /// Reference RHCR fills goals until horizon is covered. Optional — planners
    /// that don't support sequences use only `goal`.
    pub goal_sequence: SmallVec<[IVec2; 4]>,
}

// ---------------------------------------------------------------------------
// Plan fragment — result from one windowed planning call
// ---------------------------------------------------------------------------

pub struct PlanFragment {
    pub agent_index: usize,
    pub actions: SmallVec<[Action; 20]>,
}

pub enum WindowResult {
    /// All agents planned successfully.
    Solved(Vec<PlanFragment>),
    /// Some agents planned, others need fallback.
    Partial {
        solved: Vec<PlanFragment>,
        failed: Vec<usize>,
    },
}

// ---------------------------------------------------------------------------
// WindowedPlanner trait
// ---------------------------------------------------------------------------

pub trait WindowedPlanner: Send + Sync {
    fn name(&self) -> &'static str;

    fn plan_window(
        &mut self,
        ctx: &WindowContext,
        rng: &mut SeededRng,
    ) -> WindowResult;
}
