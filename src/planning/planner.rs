//! Planner — the contract every planner implements.
//!
//! Every tick, the runner calls `step()` with an `Observation`. The planner
//! decides internally whether to replan (based on its own cadence) and returns
//! either new plans or `Keep` (no work this tick).

use bevy::math::IVec2;
use smallvec::SmallVec;

use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;
use crate::core::topology::ZoneMap;

use super::heuristics::DistanceMapCache;

// ---------------------------------------------------------------------------
// Observation passed to planners each tick
// ---------------------------------------------------------------------------

pub struct Observation<'a> {
    pub tick: u64,
    /// Dead agents are obstacles in the grid.
    pub grid: &'a GridMap,
    pub zones: &'a ZoneMap,
    /// Alive agents only.
    pub agents: &'a [AgentView],
}

// ---------------------------------------------------------------------------
// Agent view — flat data, no ECS references
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AgentView {
    pub index: usize,
    pub pos: IVec2,
    pub goal: Option<IVec2>,
    pub has_plan: bool,
    pub task_leg: TaskLeg,
}

// ---------------------------------------------------------------------------
// Decision — borrows from the planner's internal buffer
// ---------------------------------------------------------------------------

/// A planner's own measurements as (key, value) pairs.
pub type Telemetry = Vec<(String, f64)>;

/// Action plan for one agent: (agent_index, actions).
pub type AgentPlan = (usize, SmallVec<[crate::core::action::Action; 20]>);

pub enum Decision<'a> {
    /// New plans — borrowed from the planner's internal buffer.
    Plans(&'a [AgentPlan]),
    /// No work this tick — keep executing current plans.
    Keep,
}

// ---------------------------------------------------------------------------
// Planner trait
// ---------------------------------------------------------------------------

pub trait Planner: Send + Sync + 'static {
    /// Called when the solver is activated or the scenario resets.
    fn reset(&mut self);

    /// Called every tick. The planner decides whether to replan.
    fn step<'a>(
        &'a mut self,
        obs: &Observation,
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> Decision<'a>;

    /// Opaque state from which `reset()` followed by `restore()` returns the
    /// planner to this tick, for rewind. `None` means the planner cannot be
    /// restored, and rewind falls back to `reset()` alone.
    fn snapshot(&self) -> Option<Vec<u8>> {
        None
    }

    /// Called after `reset()` when the run rewinds to the tick `snapshot`
    /// came from. Default: no-op.
    fn restore(&mut self, _snapshot: &[u8]) {}

    /// Drain pending goal overrides produced by the last `step()`.
    /// Planners that change goals (Token Passing sends idle agents to parking)
    /// return `(agent_index, new_goal)` pairs. The runner applies these to
    /// update `agent.goal` in the task system.
    /// Default: no overrides.
    fn goal_overrides(&mut self) -> Vec<(usize, IVec2)> {
        Vec::new()
    }

    /// Task selection owned by the solver itself. Token Passing assigns its
    /// own tasks (Ma et al. 2017); the runner then ignores the configured
    /// scheduler. Default: `None` — use the configured scheduler.
    fn task_scheduler(&self) -> Option<&dyn crate::core::task::TaskScheduler> {
        None
    }

    /// Numbers the planner reports about its own work, under keys it
    /// chooses. Read at the end of a run. Default: nothing to report.
    fn telemetry(&self) -> Telemetry {
        Vec::new()
    }
}
