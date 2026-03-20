---
name: mafis-determinism
description: >
  Determinism audit and fix skill for MAFIS timeline rewind. Use this skill whenever the
  user reports non-determinism, divergence after rewind, "simulation gives different results",
  "rewind doesn't replay the same", "timeline is wrong after resume", "agents move differently
  after rewind", "results changed after replay", or any issue where the simulation behaves
  differently when rewound and replayed. Also use when working on snapshot recording, restore
  logic, RNG state, SeededRng, the RewindRequest system, FullTickSnapshot, or restore_world_state.
  Trigger proactively when adding any new system that consumes RNG or stores state that persists
  across ticks — these are the most common sources of determinism regression. This skill contains
  the complete chain of state that must be captured and restored for bit-exact replay.
---

# MAFIS Determinism Audit

When the simulation rewinds to tick T and resumes, it must produce **identical** results to
the original run from tick T forward. This requires restoring every piece of state that
influences computation.

## The Complete State Chain

Every item below must be captured in `FullTickSnapshot` and restored in `restore_world_state`.
If ANY is missing, the simulation diverges.

### 1. RNG Stream Position (most common culprit)

**Capture** (`record_tick_snapshot` in `analysis/history.rs`):
```rust
rng_word_pos: rng.rng.get_word_pos(),
```

**Restore** (`restore_world_state` in `fault/manual.rs`):
```rust
let original_seed = res.rng.seed();
res.rng.reseed(original_seed);
res.rng.rng.set_word_pos(snapshot.rng_word_pos);
```

The snapshot runs in `AnalysisSet::Metrics` (FixedUpdate), which is AFTER all RNG consumers:
`SimulationRunner::tick()` → fault systems. So the word position captures the
exact stream state after the tick's full RNG consumption.

**Common bug**: Using `reseed(seed + tick)` instead of `set_word_pos`. This produces a
different stream — ChaCha8's word position is NOT the same as reseeding with an offset.

### 2. Agent Planned Paths

**Capture**: `AgentSnapshot.planned_actions: Vec<u8>` — encoded via `Action::to_u8()`.
**Restore**: `agent.planned_path.extend(snap.planned_actions.iter().map(Action::from_u8))`.

The snapshot captures planned_path AFTER `lifelong_replan` fills it. On the first resumed
tick, `tick_agents` consumes this path (runs BEFORE `lifelong_replan` in the system ordering).
If the path is cleared instead of restored, all agents Wait on the first tick → positions
differ → everything diverges.

**Encoding**: 0=Wait, 1=North, 2=South, 3=East, 4=West (`Action::to_u8`/`from_u8`).

### 3. PIBT Shuffle Seed

**What**: `PibtCore::shuffle_seed` — used for deterministic tie-breaking in candidate sorting.
Increments by 1 each step. After `solver.reset()`, goes to 0.

**Fix**: `PibtCore::set_shuffle_seed(tick)` is called from solver `step()` methods to derive
the seed from the simulation tick rather than an internal counter. This makes it naturally
deterministic after rewind without needing to snapshot it.

**Affected solvers**: `PibtLifelongSolver`, `RhcrSolver` (PIBT fallback). The `PibtWindowPlanner`
resets to 0 at each window start (self-contained, no issue).

### 4. Heat / Operational Age

**Capture**: `AgentSnapshot.heat: f32`
**Restore**: `heat_state.heat = snap.heat;`

Heat determines Weibull failure probability. Wrong heat → faults fire at different ticks.
In the WearBased scenario, operational_age (movement-ticks) drives the Weibull hazard function.

### 5. Lifelong Task Count

**Capture**: `FullTickSnapshot.lifelong_tasks_completed: u64`
**Restore**: `lifelong.restore_from_snapshot(tasks_completed)` — also clears `completion_ticks`
window and sets `needs_replan = true`.

### 6. Agent Positions, Goals, Task Legs

**Capture**: `AgentSnapshot.{pos, goal, task_leg, task_leg_data}`
**Restore**: Direct field assignment + `reconstruct_task_leg()` for TaskLeg enum.

### 7. Dead/Latency Components

**Capture**: `AgentSnapshot.is_dead` (latency is transient, not restored)
**Restore**: `commands.entity(entity).insert/remove::<Dead>()`

### 8. Grid State

**Restore**: Rebuild from topology, then replay `ManualFaultLog` entries up to snapshot tick.

### 9. Solver Internal State

`solver.reset()` clears all internal state. After reset + restored positions + correct RNG,
the solver produces identical plans. Each solver's `reset()`:
- **PIBT**: Clears priorities, shuffle_seed (overridden by set_shuffle_seed)
- **RHCR**: Clears plan_buffer, prev_positions, congestion_streak. Forces immediate replan.
- **Token Passing**: Clears token, plan_buffer, agent maps. Sets initialized=false.

## Architecture Note: SimulationRunner

Fault logic (Weibull detection, heat accumulation, intermittent faults) now runs inside
`SimulationRunner::tick()`, not as separate ECS systems. Only manual fault processing
(user-initiated kills/latency/obstacles) and scheduled fault replay remain as ECS systems.
This means the RNG consumption order within a tick is deterministic by construction.

## System Ordering (Critical for Understanding)

```
FixedUpdate:
  CoreSet::Tick
    SimulationRunner::tick()    ← moves agents, consumes RNG, detects faults
    → recycle_goals             ← uses RNG (assigns tasks)
    → lifelong_replan           ← uses RNG via solver (fills planned_path)
  → FaultSet::Schedule          ← replay_manual_faults (rewind only)
  → FaultSet::Heat / FaultCheck / Replan
  → CoreSet::PostTick
    → AnalysisSet::Metrics
      → record_tick_snapshot    ← captures ALL state here (after everything)
```

The snapshot is taken at the END of the tick pipeline. When restored, the NEXT tick runs
the full pipeline starting from the correct state.

## Debugging Checklist

When determinism breaks, check in this order:

1. **Is `rng_word_pos` captured and restored?** — Most common issue.
2. **Is `planned_path` restored (not just cleared)?** — Second most common.
3. **Is `shuffle_seed` derived from tick?** — Check `set_shuffle_seed` calls.
4. **Is heat restored?** — Check `heat_state.heat = snap.heat`.
5. **Is lifelong config restored?** — Check `restore_from_snapshot`.
6. **Is the grid rebuilt correctly?** — Topology regeneration + fault log replay.
7. **Are Dead/Latency components synced?** — Insert/remove based on snapshot.
8. **Is there a new system consuming RNG that wasn't here before?** — Any new system
   in SimulationRunner::tick() or between `recycle_goals` and `record_tick_snapshot`
   that uses `SeededRng` will shift the word position.
9. **Is there new solver internal state that survives reset()?** — Check the solver's
   `reset()` method clears everything.

## Testing Determinism

To verify determinism programmatically:
1. Run simulation to tick N, record snapshot
2. Rewind to tick T < N
3. Resume and run to tick N again
4. Compare: positions, goals, task legs, heat, RNG word pos must all match

The `sim_tests/` harness can be extended for this — `SimHarness` boots headless Bevy.

## Files to Check

| File | What to look for |
|------|-----------------|
| `analysis/history.rs` | `FullTickSnapshot` fields, `record_tick_snapshot` |
| `fault/manual.rs` | `restore_world_state`, `apply_rewind`, ManualFaultLog |
| `core/seed.rs` | `SeededRng`, `reseed`, `set_word_pos` |
| `core/task.rs` | `LifelongConfig::restore_from_snapshot` |
| `core/runner.rs` | `SimulationRunner::tick()` — fault detection + RNG consumption |
| `solver/pibt_core.rs` | `shuffle_seed`, `set_shuffle_seed`, `reset` |
| `solver/pibt.rs` | `set_shuffle_seed(ctx.tick)` call |
| `solver/rhcr.rs` | `pibt_fallback.set_shuffle_seed(ctx.tick)` call |
| `solver/token_passing.rs` | `reset()` clears all state |
| `core/action.rs` | `Action::to_u8`/`from_u8` round-trip |
