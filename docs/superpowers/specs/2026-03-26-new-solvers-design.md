# New Lifelong Solvers — Design Spec

**Date:** 2026-03-26
**Goal:** Add 3 new lifelong MAPF solvers covering 3 uncovered algorithm paradigms (config-space, task-swapping, meta-guidance). Keep all 5 existing solvers untouched. Design for future extensibility — the architecture must support easy addition of 10+ more algorithms over time.

**Batch 1 Solvers:**
1. **RT-LaCAM** — Real-time configuration-space DFS (`CONFIG-SPACE`)
2. **TPTS** — Token Passing with Task Swaps (`DECENTRALIZED`)
3. **PIBT+APF** — PIBT with Artificial Potential Fields (`META` `REACTIVE`)

**Constraint:** Only natively lifelong algorithms. No RHCR-windowed wrappers for algorithms that aren't designed for tick-by-tick invocation.

---

## 1. Architecture Overview

### File Structure

```
solver/
  # Existing (untouched)
  lifelong.rs               # LifelongSolver trait, SolverContext, StepResult
  traits.rs                 # SolverInfo, Optimality, Scalability, MAPFSolver
  pibt.rs                   # PibtLifelongSolver (standalone)
  pibt_core.rs              # PibtCore (shared algorithm)
  rhcr.rs, windowed.rs      # RHCR framework + WindowedPlanner trait
  pbs_planner.rs            # PBS planner
  pibt_window_planner.rs    # PIBT-Window planner
  priority_astar_planner.rs # Priority A* planner
  token_passing.rs          # Token Passing
  astar.rs                  # Spacetime A*, ConstraintChecker trait
  heuristics.rs             # DistanceMap, DistanceMapCache
  mod.rs                    # Factory + SOLVER_NAMES registry

  # New files
  guidance.rs               # GuidanceLayer trait + GuidedSolver wrapper
  rt_lacam.rs               # RT-LaCAM solver (config-space search)
  tpts.rs                   # TPTS solver (token passing + task swaps)
  token_common.rs           # Shared token internals (extracted from token_passing.rs)
  apf_guidance.rs           # APF guidance layer (potential fields)
```

### Trait Hierarchy

```
LifelongSolver              (existing, unchanged)
├── PibtLifelongSolver      (existing)
├── RhcrSolver              (existing)
├── TokenPassingSolver       (existing)
├── LaCAMSolver             (NEW — config-space DFS)
├── TptsSolver              (NEW — token passing + task swaps)
└── GuidedSolver<G>         (NEW — wraps any LifelongSolver + GuidanceLayer)

GuidanceLayer               (NEW trait)
└── ApfGuidance             (NEW — artificial potential fields)
    (future: GgoGuidance, TrafficFlowGuidance, OnlineGgoGuidance, etc.)
```

`GuidedSolver<G>` owns a `Box<dyn LifelongSolver>` (the base) and a `G: GuidanceLayer`. It delegates `step()` to the base solver but injects guidance heuristics before each call. The factory creates `GuidedSolver<ApfGuidance>` wrapping `PibtLifelongSolver` for `"pibt+apf"`.

---

## 2. GuidanceLayer Trait

```rust
/// A guidance layer modifies heuristic weights before the base solver plans.
/// It does NOT produce plans itself — it biases the solver's decisions.
pub trait GuidanceLayer: Send + Sync + 'static {
    /// Short identifier (e.g. "apf", "ggo", "traffic_flow").
    fn name(&self) -> &'static str;

    /// Called once per replan cycle, before the base solver's step().
    /// Receives current agent states + grid, produces internal guidance state.
    fn compute_guidance(
        &mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &DistanceMapCache,
    );

    /// Query the guidance bias for a specific cell.
    /// Returns a weight modifier: negative = attractive, positive = repulsive.
    /// Solvers add this to their heuristic when choosing next moves.
    fn cell_bias(&self, pos: IVec2, agent_index: usize) -> f64;

    /// Query the guidance bias for a specific edge (default: 0.0).
    /// Override for edge-weight guidance layers like GGO.
    fn edge_bias(&self, _from: IVec2, _to: IVec2, _agent_index: usize) -> f64 { 0.0 }

    /// Reset internal state (called on solver reset).
    fn reset(&mut self);
}
```

### GuidedSolver Wrapper

```rust
pub struct GuidedSolver<G: GuidanceLayer> {
    base: Box<dyn LifelongSolver>,
    guidance: G,
    plan_buffer: Vec<AgentPlan>,
}

impl<G: GuidanceLayer> LifelongSolver for GuidedSolver<G> {
    fn name(&self) -> &'static str { /* leak a composed "base+layer" string once; acceptable for small solver count */ }
    fn info(&self) -> SolverInfo { /* inherits from base, notes guidance in description */ }
    fn reset(&mut self) { self.base.reset(); self.guidance.reset(); }
    fn step(...) -> StepResult { /* compute_guidance, then delegate to base */ }
    fn save_priorities(&self) -> Vec<f32> { self.base.save_priorities() }
    fn restore_priorities(&mut self, p: &[f32]) { self.base.restore_priorities(p); }
}
```

### PibtCore Integration

Add one method to `PibtCore` (existing methods unchanged):

```rust
pub fn one_step_guided<F>(
    &mut self,
    // ... existing params (positions, goals, dist_maps, grid, shuffle_seed) ...
    bias_fn: F,
) where F: Fn(IVec2, usize) -> f64
```

Modifies neighbor ranking from `distance_to_goal` to `distance_to_goal + bias_fn(neighbor, agent)`. Existing `one_step()` and `one_step_with_tasks()` remain unchanged.

---

## 3. RT-LaCAM Solver

Real-time LaCAM: incrementally builds configuration-space DFS across ticks with millisecond budget. Remembers search state between invocations.

### Algorithm

1. **Persistent DFS state:** Keeps config-space DFS stack across ticks. Each tick, expands nodes for up to `budget_ms` milliseconds.
2. **Partial plans:** Commits best partial plan found so far (longest collision-free prefix). Agents without a plan get PIBT fallback.
3. **Incremental improvement:** As more ticks pass, DFS explores deeper, improving plan quality. Once full solution found, commit and restart search.
4. **Zobrist hashing:** XOR of (agent, cell) random keys for O(1) config dedup.

### Structure

```rust
pub struct RtLaCAMSolver {
    // Config
    budget_ms: f64,           // Time budget per tick (e.g., 2.0ms)
    max_horizon: usize,       // Max plan length before committing

    // Persistent search state (survives across ticks)
    dfs_stack: Vec<Configuration>,
    visited: HashSet<u64>,
    best_partial: Option<Vec<AgentPlan>>,
    committed_steps: usize,

    // Output
    plan_buffer: Vec<AgentPlan>,

    // Fallback
    pibt_fallback: PibtCore,

    // Zobrist keys (generated once from seed)
    zobrist_keys: Vec<Vec<u64>>,  // [agent][cell] -> random key
}
```

### Key Details

- **Zobrist hashing:** Each (agent, cell) pair gets a random u64 (generated deterministically from SeededRng on init). Configuration hash = XOR of all agent-cell hashes. O(1) duplicate detection.
- **Constraint generation:** For each agent, neighbors sorted by `distance_map[neighbor]`. This is the "low-level" — sorted list, no search.
- **DFS expansion:** Pop config from stack. Pick first agent without a decided next move. For each candidate position, if no collision with already-decided agents, create child config and push.
- **Fallback:** If budget exhausted with no complete plan, use `PibtCore::one_step()` for all agents. If partial plan exists, commit partial + PIBT for remaining agents.
- **Auto-config:** `RtLaCAMConfig::auto(grid_area, num_agents)` computes budget_ms and max_horizon based on density.
- **Search restart:** When a full solution is committed or agent goals change, clear DFS stack and visited set.

### SolverInfo

```rust
SolverInfo {
    optimality: Optimality::Suboptimal,
    complexity: "O(budget_ms) per tick, amortized config-space DFS",
    scalability: Scalability::High,
    description: "Real-time configuration-space DFS with persistent search state",
    recommended_max_agents: None,
}
```

### Reuses
- `DistanceMapCache` — neighbor ranking heuristic
- `PibtCore` — fallback for agents without plans
- `SeededRng` — Zobrist key generation (deterministic)

---

## 4. TPTS Solver (Token Passing with Task Swaps)

Extends Token Passing with task swapping: agents can exchange goals when it reduces total path cost.

### Algorithm

Same as Token Passing, plus after standard planning phase:

1. Scan for **swap candidates**: pairs (a, b) where `cost(a->goal_b) + cost(b->goal_a) < cost(a->goal_a) + cost(b->goal_b)`.
2. If beneficial swap exists, swap their goals and replan both agents against the constraint index.
3. Swap checking bounded by `max_swap_checks` per cycle, limited to agents within `swap_radius` Manhattan distance.

### Structure

```rust
pub struct TptsSolver {
    // Core token passing (shared internals from token_common.rs)
    token: Vec<VecDeque<IVec2>>,
    master_index: MasterConstraintIndex,

    // Task swap extension
    max_swap_checks: usize,    // Limit pairwise comparisons per cycle
    swap_radius: i32,          // Only check agents within this distance

    // Output
    plan_buffer: Vec<AgentPlan>,
}
```

### Shared Internals Extraction

Extract from `token_passing.rs` into `token_common.rs`:
- `MasterConstraintIndex` (reference-counted vertex/edge constraints)
- TOKEN management (advance, sync, add/remove paths)
- Planning order logic (tasked agents first)

Both `TokenPassingSolver` and `TptsSolver` import from `token_common.rs`. Existing `TokenPassingSolver` interface unchanged.

### SolverInfo

```rust
SolverInfo {
    optimality: Optimality::Suboptimal,
    complexity: "O(n * A* + swap_checks) per replan",
    scalability: Scalability::Medium,
    description: "Decentralized sequential planning with task swapping",
    recommended_max_agents: Some(100),
}
```

### Reuses
- `MasterConstraintIndex` — from token_common.rs (extracted)
- `spacetime_astar_fast()` — same A* engine
- `DistanceMapCache` — swap cost estimation

---

## 5. Factory & Registry Updates

### Updated SOLVER_NAMES

```rust
pub const SOLVER_NAMES: &[(&str, &str)] = &[
    // Existing (unchanged)
    ("pibt", "PIBT — Priority Inheritance with Backtracking"),
    ("rhcr_pbs", "RHCR (PBS) — Rolling-Horizon with Priority-Based Search"),
    ("rhcr_pibt", "RHCR (PIBT-Window) — Rolling-Horizon with PIBT"),
    ("rhcr_priority_astar", "RHCR (Priority A*) — Rolling-Horizon with Priority A*"),
    ("token_passing", "Token Passing — Decentralized Sequential Planning"),
    // New
    ("rt_lacam", "RT-LaCAM — Real-Time Configuration-Space Search"),
    ("tpts", "TPTS — Token Passing with Task Swaps"),
    ("pibt+apf", "PIBT+APF — Priority Inheritance with Potential Fields"),
];
```

### Composition Syntax

```rust
pub fn lifelong_solver_from_name(
    name: &str,
    grid_area: usize,
    num_agents: usize,
) -> Option<Box<dyn LifelongSolver>> {
    // Check for guidance composition: "base+layer"
    if let Some((base_name, layer_name)) = name.split_once('+') {
        let base = lifelong_solver_from_name(base_name, grid_area, num_agents)?;
        let guided = match layer_name {
            "apf" => {
                let layer = ApfGuidance::new(grid_area, num_agents);
                Box::new(GuidedSolver::new(base, layer)) as Box<dyn LifelongSolver>
            }
            // future: "ggo", "traffic_flow", etc.
            _ => return None,
        };
        return Some(guided);
    }

    match name {
        // ... existing matches unchanged ...
        "rt_lacam" => Some(Box::new(RtLaCAMSolver::new(grid_area, num_agents))),
        "tpts" => Some(Box::new(TptsSolver::new())),
        _ => None,
    }
}
```

Future combinations like `"rt_lacam+ggo"` or `"rhcr_pibt+apf"` work automatically via the `split_once('+')` parser — zero factory changes needed.

### Bridge Command

Existing `set_solver` command works unchanged:
```
set_solver "rt_lacam"
set_solver "tpts"
set_solver "pibt+apf"
```

---

## 6. Determinism & Rewind Support

All new solvers must support `save_priorities()` / `restore_priorities()` for timeline rewind.

| Solver | State to Save | Restore Strategy |
|---|---|---|
| RT-LaCAM | `committed_steps` count | Clear DFS stack + visited set, let search rebuild from current positions |
| TPTS | Token paths snapshot | Rebuild master index from restored token |
| PIBT+APF | Base PIBT priorities only | Guidance is stateless (recomputed per tick from positions) |

For RT-LaCAM and TPTS, we save minimal state and let the solver rebuild search structures on restore — same pattern as existing solvers.

---

## 7. Test Coverage

Each solver gets the standard test suite:

```rust
#[cfg(test)]
mod tests {
    // 1. Basic: 1 agent, open grid, reaches goal
    // 2. Collision: 2 agents swapping positions, no vertex/edge collision
    // 3. Corridor: N agents in narrow passage, all reach goals
    // 4. Determinism: same seed -> identical plans across 100 ticks
    // 5. Reset: solver.reset() clears state, next step() works
    // 6. Fallback: RT-LaCAM with budget_ms=0 falls back to PIBT cleanly
    // 7. Task swap: TPTS swaps when beneficial (cost comparison test)
    // 8. Guidance: PIBT+APF produces different (better) plans than vanilla PIBT
    // 9. Factory: lifelong_solver_from_name("rt_lacam"|"tpts") returns Some
    // 10. Composition: lifelong_solver_from_name("pibt+apf") returns Some
    // 11. Composition parse: "unknown+apf" returns None, "pibt+unknown" returns None
}
```

---

## 8. Future Extensibility

This architecture supports adding future algorithms with minimal changes:

### Adding a new core solver (e.g., LSRP, Causal PIBT)
1. Create `solver/lsrp.rs` implementing `LifelongSolver`
2. Add entry to `SOLVER_NAMES` in `mod.rs`
3. Add match arm in `lifelong_solver_from_name()`

### Adding a new guidance layer (e.g., GGO, Traffic Flow)
1. Create `solver/ggo_guidance.rs` implementing `GuidanceLayer`
2. Add match arm in the `layer_name` match inside the factory
3. All existing base solvers are automatically composable with it

### Adding a new paradigm (e.g., LNS improvement layer)
The `GuidanceLayer` trait covers heuristic bias. For post-processing improvement layers (LNS), a separate `ImprovementLayer` trait could be added later without changing existing code.

---

## Algorithm Reference

### Existing Solvers (5, unchanged)

| Solver | Tags | Optimality | Scale | Paradigm |
|---|---|---|---|---|
| PIBT | `REACTIVE` | Suboptimal | High (1000+) | Per-agent priority inheritance |
| RHCR (PBS) | `WINDOWED` | Suboptimal | Medium | Windowed priority-based search |
| RHCR (PIBT-Window) | `WINDOWED` `REACTIVE` | Suboptimal | High | Windowed unrolled PIBT |
| RHCR (Priority A*) | `WINDOWED` | Suboptimal | Medium | Windowed sequential A* |
| Token Passing | `DECENTRALIZED` | Suboptimal | Medium (100) | Sequential planning via shared token |

### New Solvers (3, this batch)

| Solver | Tags | Optimality | Scale | Paradigm |
|---|---|---|---|---|
| RT-LaCAM | `CONFIG-SPACE` | Suboptimal | High (1000+) | Real-time config-space DFS |
| TPTS | `DECENTRALIZED` | Suboptimal | Medium (100) | Token passing + task swaps |
| PIBT+APF | `META` `REACTIVE` | Suboptimal | High (1000+) | Potential field guidance over PIBT |

### Papers

- **RT-LaCAM:** arXiv:2504.06091, SoCS 2025 — "Real-Time LaCAM"
- **TPTS:** Ma, Li et al., AAMAS 2017 — "Lifelong Multi-Agent Path Finding for Online Pickup and Delivery Tasks"
- **PIBT+APF:** arXiv:2505.22753, May 2025 — "PIBT with Artificial Potential Fields for Lifelong MAPF"
- **GuidanceLayer design informed by:** GGO (Zhang et al., IJCAI 2024), Guided-PIBT (Chen et al., AAAI 2024)
