//! LifelongSolver — the single trait for all lifelong MAPF solvers.
//!
//! Every tick, the ECS system calls `step()`. The solver decides internally
//! whether to replan (based on its own cadence) and returns either new plans
//! or `Continue` (no work this tick).

use bevy::prelude::*;
use smallvec::SmallVec;

use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::task::TaskLeg;
use crate::core::topology::ZoneMap;

use super::heuristics::DistanceMapCache;
use super::traits::SolverInfo;

// ---------------------------------------------------------------------------
// Context passed to solvers each tick
// ---------------------------------------------------------------------------

pub struct SolverContext<'a> {
    pub grid: &'a GridMap,
    pub zones: &'a ZoneMap,
    pub tick: u64,
    pub num_agents: usize,
}

// ---------------------------------------------------------------------------
// Agent snapshot — flat data, no ECS references
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AgentState {
    pub index: usize,
    pub pos: IVec2,
    pub goal: Option<IVec2>,
    pub has_plan: bool,
    pub task_leg: TaskLeg,
}

// ---------------------------------------------------------------------------
// Step result — borrows from solver's internal buffer
// ---------------------------------------------------------------------------

/// Action plan for one agent: (agent_index, actions).
pub type AgentPlan = (usize, SmallVec<[crate::core::action::Action; 20]>);

pub enum StepResult<'a> {
    /// Solver produced new plans — borrow from its internal buffer.
    Replan(&'a [AgentPlan]),
    /// No work this tick — keep executing current plans.
    Continue,
}

// ---------------------------------------------------------------------------
// LifelongSolver trait
// ---------------------------------------------------------------------------

pub trait LifelongSolver: Send + Sync + 'static {
    /// Short identifier (e.g. `"pibt"`, `"rhcr_pbs"`).
    fn name(&self) -> &'static str;

    /// Structured metadata.
    fn info(&self) -> SolverInfo;

    /// Called when the solver is activated or the scenario resets.
    fn reset(&mut self);

    /// Called every tick. Solver decides whether to replan.
    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a>;

    /// Save internal priority state for deterministic rewind.
    /// Default: no state to save.
    fn save_priorities(&self) -> Vec<f32> { Vec::new() }

    /// Restore internal priority state from a snapshot.
    /// Default: no-op (solver reinitializes on next step).
    fn restore_priorities(&mut self, _priorities: &[f32]) {}

    /// Set a cell-level heuristic bias function for guided planning.
    /// Solvers that support guidance override this. Default: no-op.
    fn set_cell_bias(&mut self, _bias: Option<Box<dyn Fn(IVec2, usize) -> f64 + Send + Sync>>) {}

    /// Drain pending goal overrides produced by the last `step()`.
    /// Solvers that swap goals (e.g. TPTS) return `(agent_index, new_goal)` pairs.
    /// The runner applies these to update `agent.goal` in the task system.
    /// Default: no overrides.
    fn drain_goal_overrides(&mut self) -> Vec<(usize, IVec2)> { Vec::new() }
}

// ---------------------------------------------------------------------------
// ActiveSolver resource
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ActiveSolver {
    solver: Box<dyn LifelongSolver>,
    name: String,
}

impl ActiveSolver {
    pub fn new(solver: Box<dyn LifelongSolver>) -> Self {
        let name = solver.name().to_string();
        Self { solver, name }
    }

    pub fn solver(&self) -> &dyn LifelongSolver {
        self.solver.as_ref()
    }

    pub fn solver_mut(&mut self) -> &mut dyn LifelongSolver {
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
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        self.solver.step(ctx, agents, distance_cache, rng)
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
}
