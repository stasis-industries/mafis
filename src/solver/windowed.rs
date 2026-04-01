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
    /// Warm-start initial plans from previous replan (agent-aligned).
    /// `initial_plans[local_idx] = Some(tail_plan)` if the previous plan is
    /// still valid for that agent, `None` otherwise.
    pub initial_plans: Vec<Option<Vec<Action>>>,
    /// Vertex constraints at t=0 for all agents' start positions.
    /// Prevents agent A from planning to be at agent B's position at t=0.
    pub start_constraints: Vec<(IVec2, u64)>,
    /// Per-cell travel penalties (grid-aligned flat array, indexed by y*width+x).
    /// Higher values indicate historically congested cells. Planners may use
    /// this as an additive cost bias. Empty slice = no penalties.
    pub travel_penalties: &'a [f32],
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

    /// Reset all internal state (for rewind/scenario change).
    /// Default: no-op.
    fn reset(&mut self) {}

    /// Save internal priority state for deterministic rewind.
    /// Default: no state to save.
    fn save_priorities(&self) -> Vec<f32> { Vec::new() }

    /// Restore priorities from a snapshot.
    /// Default: no-op.
    fn restore_priorities(&mut self, _priorities: &[f32]) {}
}
