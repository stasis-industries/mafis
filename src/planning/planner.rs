//! Planner — the contract every planner implements.
//!
//! Every tick, the runner calls `step()` with an `Observation`. The planner
//! decides internally whether to replan (based on its own cadence) and returns
//! either new plans or `Keep` (no work this tick).

use bevy::ecs::resource::Resource;
use bevy::math::IVec2;
use smallvec::SmallVec;

use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;
use crate::core::topology::ZoneMap;

use super::heuristics::DistanceMapCache;
use super::traits::SolverInfo;

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

/// Per-agent data passed to `Planner::restore_state` during rewind.
///
/// Bundles the subset of `AgentView` needed to reconstruct planner-internal
/// planning caches (e.g. Token Passing's per-agent token paths) from the
/// restored snapshot.
#[derive(Clone, Debug)]
pub struct RestoredAgent<'a> {
    pub index: usize,
    pub pos: IVec2,
    pub goal: Option<IVec2>,
    pub task_leg: TaskLeg,
    pub planned_actions: &'a [crate::core::action::Action],
}

// ---------------------------------------------------------------------------
// Decision — borrows from the planner's internal buffer
// ---------------------------------------------------------------------------

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
    /// Short identifier (e.g. `"pibt"`, `"rhcr_pbs"`).
    fn name(&self) -> &'static str;

    /// Structured metadata.
    fn info(&self) -> SolverInfo;

    /// Called when the solver is activated or the scenario resets.
    fn reset(&mut self);

    /// Called every tick. The planner decides whether to replan.
    fn step<'a>(
        &'a mut self,
        obs: &Observation,
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> Decision<'a>;

    /// Save internal priority state for deterministic rewind.
    /// Default: no state to save.
    fn save_priorities(&self) -> Vec<f32> {
        Vec::new()
    }

    /// Restore internal priority state from a snapshot.
    /// Default: no-op (solver reinitializes on next step).
    fn restore_priorities(&mut self, _priorities: &[f32]) {}

    /// Rebuild solver-specific planning caches from the post-rewind agent
    /// state + their restored planned actions. Called by the rewind machinery
    /// AFTER `reset()` + `restore_priorities()` so the solver can use the
    /// re-materialised plans to re-seed any state that `reset()` just cleared.
    ///
    /// Required to keep Token Passing deterministic across rewinds: TP's
    /// `MasterConstraintIndex` is rebuilt every step from the per-agent token
    /// paths, so losing the paths on reset makes subsequent A* plans diverge
    /// from the original run.
    ///
    /// Default: no-op (PIBT, RHCR-PBS have no path-based state to restore —
    /// they replan fresh each tick or each window).
    fn restore_state(&mut self, _agents: &[RestoredAgent]) {}

    /// Drain pending goal overrides produced by the last `step()`.
    /// Planners that change goals (Token Passing sends idle agents to parking)
    /// return `(agent_index, new_goal)` pairs. The runner applies these to
    /// update `agent.goal` in the task system.
    /// Default: no overrides.
    fn goal_overrides(&mut self) -> Vec<(usize, IVec2)> {
        Vec::new()
    }

    /// Largest fleet this solver supports on the given map. Token Passing
    /// derives it from the instance (one parking endpoint per agent, Ma et
    /// al. 2017 well-formedness condition 2). Default: the static
    /// `info().recommended_max_agents`.
    fn max_agents(&self, _zones: &crate::core::topology::ZoneMap) -> Option<usize> {
        self.info().recommended_max_agents
    }

    /// Task selection owned by the solver itself. Token Passing assigns its
    /// own tasks (Ma et al. 2017); the runner then ignores the configured
    /// scheduler. Default: `None` — use the configured scheduler.
    fn task_scheduler(&self) -> Option<&dyn crate::core::task::TaskScheduler> {
        None
    }

    /// Solver-specific telemetry: fraction of replan windows that could not
    /// close the full fleet plan within the node budget. RHCR-PBS overrides
    /// this to expose the `WindowResult::Partial` fire rate — used as an
    /// observatory probe for PBS scalability cliffs. Default: no telemetry.
    fn pbs_partial_rate(&self) -> Option<f32> {
        None
    }

    /// RHCR telemetry: fraction of replan windows in which more than half the
    /// agents never moved (reference `BasicSystem::congested`, where the
    /// reference run would stop). Default: no telemetry.
    fn congested_window_rate(&self) -> Option<f32> {
        None
    }

    /// RHCR telemetry: plans made between windows for agents left without a
    /// plan (new goal, runner-forced wait, fault). Default: no telemetry.
    fn extra_replan_count(&self) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------------------
// ActiveSolver resource
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ActiveSolver {
    solver: Box<dyn Planner>,
    name: String,
}

impl ActiveSolver {
    pub fn new(solver: Box<dyn Planner>) -> Self {
        let name = solver.name().to_string();
        Self { solver, name }
    }

    pub fn solver(&self) -> &dyn Planner {
        self.solver.as_ref()
    }

    pub fn solver_mut(&mut self) -> &mut dyn Planner {
        self.solver.as_mut()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn info(&self) -> SolverInfo {
        self.solver.info()
    }

    pub fn step<'a>(
        &'a mut self,
        obs: &Observation,
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> Decision<'a> {
        self.solver.step(obs, distance_cache, rng)
    }

    pub fn reset(&mut self) {
        self.solver.reset();
    }

    pub fn save_priorities(&self) -> Vec<f32> {
        self.solver.save_priorities()
    }

    pub fn restore_priorities(&mut self, priorities: &[f32]) {
        self.solver.restore_priorities(priorities);
    }

    pub fn restore_state(&mut self, agents: &[RestoredAgent]) {
        self.solver.restore_state(agents);
    }
}
