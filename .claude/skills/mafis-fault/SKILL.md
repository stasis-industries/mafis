---
name: mafis-fault
description: >
  Fault injection system guide for MAFIS. Use this skill when the user wants to add a new
  fault type, modify heat/wear accumulation, change fault scenarios, work on the Weibull model,
  add manual fault injection, modify the FaultSchedule, change how breakdowns work, add latency
  injection, work on fault detection thresholds, or modify the heat-to-failure pipeline. Also
  trigger for: "add a fault", "modify heat", "change the wear model", "fault scenario", "kill
  an agent", "inject latency", "fault schedule", "Weibull", "breakdown", "overheat", "fault
  config", "intermittent fault", "burst failure", "zone outage", "fault type", or any work in
  src/fault/. This skill contains the complete fault architecture: types, scenarios, Weibull
  wear model, manual injection, scheduled events, rewind integration, and the ECS system ordering.
---

# MAFIS Fault Injection System

The fault system models how multi-agent systems degrade under realistic failure conditions.
It is the core research mechanism — MAFIS exists to measure fault resilience.

## Architecture

```
src/fault/
├── mod.rs        ← FaultPlugin, FaultSet ordering (Schedule → Heat → FaultCheck → Replan)
├── config.rs     ← FaultConfig resource, FaultType enum, FaultSource enum
├── scenario.rs   ← FaultScenarioType (4 types), WearHeatRate, FaultSchedule
├── heat.rs       ← HeatState component (heat: f32, total_moves: u32)
├── breakdown.rs  ← Dead component (SparseSet), FaultEvent message, LatencyFault component
└── manual.rs     ← ManualFaultCommand message, ManualFaultLog, RewindRequest
```

## Fault Types

```rust
pub enum FaultType {
    Overheat,    // Heat-based threshold exceeded
    Breakdown,   // Permanent failure (Dead component)
    Latency,     // Temporary unavailability (forces Wait for N ticks)
}

pub enum FaultSource {
    Automatic,   // Detected by SimulationRunner (Weibull, intermittent)
    Manual,      // User-initiated via UI/bridge
    Scheduled,   // From FaultSchedule (scenario events)
}
```

## Fault Scenarios (4 types)

```rust
pub enum FaultScenarioType {
    BurstFailure,       // Kill X% of robots at tick T
    WearBased,          // Weibull wear model — busiest robots fail
    ZoneOutage,         // Latency on all robots in highest-traffic zone
    IntermittentFault,  // Exponential inter-arrival temporary faults
}
```

### BurstFailure
- Kills `burst_fraction` of agents at `burst_tick`
- Simple, catastrophic — tests worst-case robustness
- Parameters: `burst_fraction: f32` (0.0-1.0), `burst_tick: u64`

### WearBased (Weibull Model)
- Each agent accumulates `operational_age` (movement-ticks)
- Hazard rate: `h(t) = (beta/eta) * (t/eta)^(beta-1)` per tick
- Beta > 1 = wear-out phase (accelerating failure rate)
- Literature calibrated: Carlson & Murphy 2006, CASUN AGV studies

| Preset | Beta | Eta | ~MTTF | ~% dead at tick 500 |
|--------|------|-----|-------|---------------------|
| Low | 2.0 | 900 | ~800t | ~27% |
| Medium | 2.5 | 500 | ~445t | ~63% |
| High | 3.5 | 150 | ~137t | ~90% |

### ZoneOutage
- Identifies highest-traffic zone via heatmap
- Injects latency on all agents in that zone for N ticks
- Tests spatial fault tolerance and rerouting

### IntermittentFault
- Each agent independently samples next fault from `Exp(1/mtbf_ticks)`
- Faults inject latency (not death) — agent recovers after `recovery_ticks`
- Models transient disruptions (sensor glitches, communication drops)
- Parameters: `intermittent_mtbf_ticks: u64`, `intermittent_recovery_ticks: u32`

## FaultConfig Resource

```rust
pub struct FaultConfig {
    pub enabled: bool,
    // Weibull wear model
    pub weibull_enabled: bool,
    pub weibull_beta: f32,       // shape (>1 = wear-out)
    pub weibull_eta: f32,        // scale (characteristic life in movement-ticks)
    // Intermittent fault model
    pub intermittent_enabled: bool,
    pub intermittent_mtbf_ticks: u64,
    pub intermittent_recovery_ticks: u32,
}
```

## Components & Messages

### Dead (breakdown.rs)
```rust
#[derive(Component)]
#[component(storage = "SparseSet")]  // Sparse — most agents are alive
pub struct Dead;
```

When an agent dies: `Dead` is inserted, an obstacle is placed at its position, the solver excludes it.

### LatencyFault (breakdown.rs)
```rust
#[derive(Component)]
pub struct LatencyFault { pub remaining: u32 }
```
Forces the agent to Wait each tick. `remaining` decrements until 0, then the component is removed. The agent resumes normal operation.

### FaultEvent (breakdown.rs)
```rust
#[derive(Message)]
pub struct FaultEvent {
    pub entity: Entity,
    pub fault_type: FaultType,
    pub source: FaultSource,
    pub tick: u64,
    pub position: IVec2,
}
```

### ManualFaultCommand (manual.rs)
```rust
#[derive(Message)]
pub enum ManualFaultCommand {
    KillAgent(usize),
    PlaceObstacle(IVec2),
    InjectLatency { agent_id: usize, duration: u32 },
    KillAgentScheduled(usize),        // From FaultSchedule
    InjectLatencyScheduled { agent_id: usize, duration: u32 },
}
```

## System Ordering

```
FixedUpdate:
  CoreSet::Tick
    SimulationRunner::tick()   ← detects Weibull faults + intermittent faults internally
  → FaultSet::Schedule         ← replay_manual_faults (rewind replay only)
  → FaultSet::Heat             ← (reserved, currently handled in SimulationRunner)
  → FaultSet::FaultCheck       ← (reserved)
  → FaultSet::Replan           ← (reserved)
  → CoreSet::PostTick

Update:
  process_manual_faults        ← drains ManualFaultCommand messages, applies kills/latency/obstacles
  apply_rewind                 ← processes RewindRequest, restores world state
  (both run .after(BridgeSet))
```

**Key architecture note**: Automatic fault detection (Weibull, intermittent) now runs inside `SimulationRunner::tick()`, not as separate ECS systems. This ensures deterministic RNG consumption order. Only manual/scheduled fault processing remains as ECS systems.

## ManualFaultLog & Rewind

```rust
pub struct ManualFaultLog {
    pub entries: Vec<ManualFaultEntry>,
    pub replay_from: Option<usize>,  // Set by restore_world_state
}

pub struct ManualFaultEntry {
    pub tick: u64,
    pub action: ManualFaultAction,
    pub source: FaultSource,
}
```

On rewind:
1. `restore_world_state` rebuilds grid from topology
2. `ManualFaultLog` entries up to snapshot tick are replayed via `replay_manual_faults`
3. Entries after the rewind point are truncated (`truncate_after_tick`)
4. `DeleteFaultAtTick` removes the entry, then rewinds

## FaultSchedule

```rust
pub struct FaultSchedule {
    pub events: Vec<ScheduledEvent>,
}
```

Built by `FaultScenario::build_schedule()` when a scenario is activated. Events are processed at their target ticks by `replay_manual_faults` using `ManualFaultCommand::*Scheduled` variants.

## Adding a New Fault Type

1. Add variant to `FaultType` enum in `config.rs`
2. Add detection logic in `SimulationRunner::tick()` (for automatic faults) or as a `ManualFaultCommand` variant (for user-triggered)
3. Add scenario parameters to `FaultConfig` if needed
4. Add ECS component if the fault has per-agent state (like `LatencyFault`)
5. Ensure `ManualFaultLog` captures the action for rewind replay
6. Add scenario type to `FaultScenarioType` if it's a new research scenario
7. Update bridge/desktop UI to expose the new parameters
8. Write tests in `src/fault/scenario.rs` and integration tests via `SimHarness`

## Common Pitfalls

| Pitfall | Solution |
|---------|----------|
| Fault not replaying after rewind | Check ManualFaultLog captures the action with correct tick |
| Weibull fires at wrong time after rewind | Ensure heat/operational_age is restored in snapshot |
| Intermittent faults not deterministic | Must consume RNG in consistent order within SimulationRunner |
| Dead agent still blocking after restart | Grid must be rebuilt from topology, obstacles re-placed from log |
| LatencyFault persists forever | Check remaining decrements and component is removed at 0 |
