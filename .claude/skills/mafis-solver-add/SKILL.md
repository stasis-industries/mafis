---
name: mafis-solver-add
description: >
  Checklist and template for adding new lifelong MAPF solvers to MAFIS. Use this skill
  when the user wants to add a new solver, implement a new algorithm, create a new planner,
  implement a new pathfinding approach, or asks about the LifelongSolver trait. Also trigger
  when the user mentions RHCR variants, new windowed planners, "add a second solver", "how
  does the solver integrate", or wants to understand how solvers plug into the ECS tick loop.
  This skill ensures every new solver follows the architecture: trait compliance, factory
  registration, proper reset(), determinism support, and test coverage. Do NOT trigger for
  modifying existing solver logic or fixing solver bugs — this is specifically for adding
  NEW solvers.
---

# Adding a New Lifelong Solver to MAFIS

MAFIS is a fault resilience observatory. Only **lifelong-capable** solvers belong.
One-shot solvers (CBS, LaCAM, PBS, LNS2) are archived on `archive/one-shot-solvers`.

## Architecture Overview

```
lifelong.rs          ← LifelongSolver trait + ActiveSolver resource
├── pibt.rs          ← PibtLifelongSolver (wraps PibtCore)
├── rhcr.rs          ← RhcrSolver (wraps WindowedPlanner + PibtCore fallback)
│   ├── pbs_planner.rs
│   ├── pibt_window_planner.rs
│   └── priority_astar_planner.rs
├── token_passing.rs ← TokenPassingSolver
└── pibt_core.rs     ← Shared PIBT algorithm (used by multiple solvers)
```

5 solvers currently registered: `pibt`, `rhcr_pbs`, `rhcr_pibt`, `rhcr_priority_astar`, `token_passing`.

## Step-by-Step Checklist

### 1. Implement the LifelongSolver Trait

Create `src/solver/your_solver.rs`:

```rust
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use super::heuristics::DistanceMapCache;
use super::lifelong::*;
use super::traits::{Optimality, Scalability, SolverInfo};

pub struct YourSolver {
    plan_buffer: Vec<AgentPlan>,
    // Your internal state here
}

impl LifelongSolver for YourSolver {
    fn name(&self) -> &'static str { "your_solver" }

    fn info(&self) -> SolverInfo {
        SolverInfo {
            optimality: Optimality::Suboptimal,
            complexity: "O(...) per timestep",
            scalability: Scalability::High,
            description: "...",
            recommended_max_agents: Some(200),
        }
    }

    fn reset(&mut self) {
        self.plan_buffer.clear();
        // CRITICAL: Clear ALL internal state. After rewind, the solver must
        // produce identical results given the same positions + RNG.
    }

    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        // Decide whether to replan this tick
        // Build plans into self.plan_buffer
        // Return StepResult::Replan(&self.plan_buffer) or StepResult::Continue
    }
}
```

### 2. Key Design Decisions

**Replanning cadence**: How often does the solver replan?
- Every tick (like PIBT) — reactive but more compute
- Every W ticks (like RHCR) — amortized cost, uses a window
- On-demand (like Token Passing) — only when agents need new paths

**RNG usage**: If your solver uses randomness, consume from `rng: &mut SeededRng`.
Never use `thread_rng()` or system randomness — breaks deterministic replay.

**Pre-allocated buffers**: Use `self.plan_buffer: Vec<AgentPlan>` for zero-allocation replans.
The `step()` method returns a borrow `&[AgentPlan]` from this buffer.

### 3. Register in the Factory

In `src/solver/mod.rs`, add to `lifelong_solver_from_name`:

```rust
"your_solver" => Some(Box::new(YourSolver::new(/* params */))),
```

### 4. Add Module

In `src/solver/mod.rs`:
```rust
pub mod your_solver;
```

### 5. Add Constants (if needed)

All tunable limits go in `src/constants.rs`. Never hardcode magic numbers.

### 6. Write Tests

Required test coverage:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_empty_agents() { /* Must handle 0 agents without panic */ }

    #[test]
    fn solver_single_agent_reaches_goal() { /* 1 agent, verify it plans toward goal */ }

    #[test]
    fn solver_two_agents_no_collision() { /* 2 crossing agents, no vertex conflicts */ }

    #[test]
    fn solver_warehouse_no_obstacle_violations() { /* Warehouse topology, no obstacle entry */ }

    #[test]
    fn solver_reset_clears_state() { /* After reset(), internal state is clean */ }
}
```

For integration tests using `SimHarness` (in `src/sim_tests/`):
```rust
#[test]
fn solver_warehouse_lifelong_no_obstacle_violations() {
    let mut h = SimHarness::new(8, "warehouse_small");
    h.set_solver("your_solver");
    h.run_ticks(200);
}
```

### 7. UI Integration

- **WASM bridge**: Already handles any registered solver via `set_solver "name"`. No changes needed.
- **Desktop egui**: Add entry in `src/ui/desktop/panels/solver.rs` solver picker ComboBox.

### 8. Determinism Compliance

Your solver MUST be deterministic given same agent positions + same RNG state + same grid.
After `reset()` + restored state, the solver must produce identical plans.

Never use: `HashMap` iteration order (use sorted keys or `BTreeMap`), system time,
thread-local state, or any non-deterministic source.

## RHCR Variant (Windowed Planner)

If adding a new RHCR windowed planner:

1. Implement `WindowedPlanner` trait:
```rust
impl WindowedPlanner for YourPlanner {
    fn name(&self) -> &'static str { "your_planner" }
    fn plan_window(&mut self, ctx: &WindowContext, rng: &mut SeededRng) -> WindowResult { ... }
}
```

2. Add to `RhcrMode` enum and `RhcrSolver::new()` factory.
3. Register as `"rhcr_your_planner"` in `lifelong_solver_from_name`.
4. `RhcrConfig::auto()` should have smart defaults for your planner.

## Common Pitfalls

| Pitfall | Solution |
|---------|----------|
| `reset()` doesn't clear everything | List every field, clear each one explicitly |
| Using `HashMap` iteration | Use `BTreeMap` or sort keys before iterating |
| Hardcoded numbers in solver | Move to `constants.rs` |
| Not handling dead agents | Dead agents are excluded from `agents` slice |
| O(n²) in agent loop | Use `HashSet` or pre-built index for lookups |
| Plans reference wrong agent index | Use `agents[i].index` (global), not `i` (local) |
