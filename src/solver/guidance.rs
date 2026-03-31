//! GuidanceLayer — composable meta-layer for solver heuristic bias.
//!
//! A GuidanceLayer modifies heuristic weights before the base solver plans.
//! It does NOT produce plans itself — it biases the solver's decisions.
//!
//! Use via GuidedSolver<G>: wraps any LifelongSolver + GuidanceLayer.
//! Factory creates these via "base+layer" syntax (e.g., "pibt+ggo").
//! Note: PIBT+APF is NOT a GuidedSolver — it requires sequential APF
//! update inside PIBT recursion, so it uses PibtApfSolver directly.

use bevy::prelude::*;

use crate::core::seed::SeededRng;

use super::heuristics::DistanceMapCache;
use super::lifelong::{AgentState, LifelongSolver, SolverContext, StepResult};
use super::traits::SolverInfo;

// ---------------------------------------------------------------------------
// GuidanceLayer trait
// ---------------------------------------------------------------------------

/// A guidance layer biases solver decisions via cell/edge weights.
/// Compute once per tick, query per candidate cell.
pub trait GuidanceLayer: Send + Sync + 'static {
    /// Short identifier (e.g. "apf", "ggo").
    fn name(&self) -> &'static str;

    /// Called once per tick before the base solver's step().
    fn compute_guidance(
        &mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &DistanceMapCache,
    );

    /// Cell-level heuristic bias. Negative = attractive, positive = repulsive.
    fn cell_bias(&self, pos: IVec2, agent_index: usize) -> f64;

    /// Edge-level heuristic bias (default: 0.0).
    fn edge_bias(&self, _from: IVec2, _to: IVec2, _agent_index: usize) -> f64 {
        0.0
    }

    /// Reset internal state.
    fn reset(&mut self);
}

// ---------------------------------------------------------------------------
// GuidedSolver<G> — wraps LifelongSolver + GuidanceLayer
// ---------------------------------------------------------------------------

/// Wraps any `LifelongSolver` with a `GuidanceLayer` that biases its planning.
pub struct GuidedSolver<G: GuidanceLayer> {
    base: Box<dyn LifelongSolver>,
    guidance: G,
    name: &'static str, // leaked once in new()
}

impl<G: GuidanceLayer> GuidedSolver<G> {
    pub fn new(base: Box<dyn LifelongSolver>, guidance: G) -> Self {
        let composed = format!("{}+{}", base.name(), guidance.name());
        let name: &'static str = Box::leak(composed.into_boxed_str());
        Self { base, guidance, name }
    }
}

impl<G: GuidanceLayer> LifelongSolver for GuidedSolver<G> {
    fn name(&self) -> &'static str {
        self.name
    }

    fn info(&self) -> SolverInfo {
        let base_info = self.base.info();
        SolverInfo {
            optimality: base_info.optimality,
            complexity: base_info.complexity,
            scalability: base_info.scalability,
            description: base_info.description,
            source: base_info.source,
            recommended_max_agents: base_info.recommended_max_agents,
        }
    }

    fn reset(&mut self) {
        self.base.set_cell_bias(None);
        self.base.reset();
        self.guidance.reset();
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        // Compute guidance
        self.guidance.compute_guidance(ctx, agents, distance_cache);

        // Build a boxed closure that queries the guidance layer.
        // Safety: guidance_ptr_addr is the address of self.guidance, which is
        // valid for the lifetime of this GuidedSolver (which outlives the
        // closure passed to set_cell_bias). We store it as usize to avoid
        // the raw-pointer Send + Sync restrictions — G: Send + Sync so this
        // is safe as long as the closure does not outlive self.
        let guidance_ptr_addr = &self.guidance as *const G as usize;
        let bias_fn: Box<dyn Fn(IVec2, usize) -> f64 + Send + Sync> =
            Box::new(move |pos, agent| unsafe {
                (*(guidance_ptr_addr as *const G)).cell_bias(pos, agent)
            });

        self.base.set_cell_bias(Some(bias_fn));
        self.base.step(ctx, agents, distance_cache, rng)
    }

    fn save_priorities(&self) -> Vec<f32> {
        self.base.save_priorities()
    }

    fn restore_priorities(&mut self, priorities: &[f32]) {
        self.base.restore_priorities(priorities);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::core::topology::ZoneMap;
    use crate::solver::pibt::PibtLifelongSolver;
    use crate::solver::traits::{Optimality, Scalability};
    use std::collections::HashMap;

    struct NullGuidance;

    impl GuidanceLayer for NullGuidance {
        fn name(&self) -> &'static str { "null" }
        fn compute_guidance(&mut self, _: &SolverContext, _: &[AgentState], _: &DistanceMapCache) {}
        fn cell_bias(&self, _pos: IVec2, _agent: usize) -> f64 { 0.0 }
        fn reset(&mut self) {}
    }

    fn test_zones() -> ZoneMap {
        ZoneMap {
            pickup_cells: vec![IVec2::new(0, 0)],
            delivery_cells: vec![IVec2::new(4, 4)],
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: HashMap::new(),
            queue_lines: Vec::new(),
        }
    }

    #[test]
    fn guided_solver_name_is_composed() {
        let base = Box::new(PibtLifelongSolver::new());
        let guided = GuidedSolver::new(base, NullGuidance);
        assert_eq!(guided.name(), "pibt+null");
    }

    #[test]
    fn guided_solver_delegates_step() {
        let base = Box::new(PibtLifelongSolver::new());
        let mut guided = GuidedSolver::new(base, NullGuidance);
        let grid = GridMap::new(5, 5);
        let zones = test_zones();
        let mut cache = DistanceMapCache::default();
        let mut rng = SeededRng::new(42);
        let ctx = SolverContext { grid: &grid, zones: &zones, tick: 0, num_agents: 0 };

        let result = guided.step(&ctx, &[], &mut cache, &mut rng);
        assert!(matches!(result, StepResult::Replan(plans) if plans.is_empty()));
    }

    #[test]
    fn guided_solver_reset_clears_both() {
        let base = Box::new(PibtLifelongSolver::new());
        let mut guided = GuidedSolver::new(base, NullGuidance);
        guided.reset();
    }

    #[test]
    fn guided_solver_inherits_info() {
        let base = Box::new(PibtLifelongSolver::new());
        let guided = GuidedSolver::new(base, NullGuidance);
        let info = guided.info();
        assert_eq!(info.optimality, Optimality::Suboptimal);
        assert_eq!(info.scalability, Scalability::High);
    }
}
