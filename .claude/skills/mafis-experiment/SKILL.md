---
name: mafis-experiment
description: >
  Guide for running experiments and exporting data from MAFIS. Use this skill when the
  user wants to run an experiment, compare configurations, do a parameter sweep, batch run
  multiple scenarios, export simulation data, analyze results across runs, set up a baseline
  comparison, or work with the headless baseline engine. Also trigger for: "run experiment",
  "compare solvers", "batch run", "parameter sweep", "export CSV", "export JSON", "export
  results", "run headless", "baseline comparison", "which config is better", "collect data",
  "research protocol", or any mention of systematic comparison between simulation configurations.
  This skill contains the headless engine API, export system, experiment design patterns, and
  the configuration matrix approach for fault resilience research.
---

# MAFIS Experiment Guide

MAFIS is a fault resilience observatory. The primary research workflow is:
1. Configure a scenario (topology + agents + solver + scheduler + faults)
2. Run the simulation (live or headless)
3. Collect metrics
4. Compare against baseline or alternative configurations

## Headless Baseline Engine (src/analysis/baseline.rs)

The headless engine runs a pure-Rust simulation without rendering — identical logic, no Bevy overhead. Used for baseline comparison and batch experiments.

### API

```rust
pub struct BaselineConfig {
    pub topology_name: String,
    pub num_agents: usize,
    pub solver_name: String,
    pub scheduler_name: String,
    pub seed: u64,
    pub tick_count: u64,
}

pub struct BaselineRecord {
    pub throughput_series: Vec<f64>,
    pub tasks_completed_series: Vec<u64>,
    pub idle_count_series: Vec<usize>,
    pub traffic_counts: HashMap<IVec2, u32>,
    pub total_tasks: u64,
    pub avg_throughput: f64,
    pub peak_throughput: f64,
}

pub fn run_headless(config: BaselineConfig) -> BaselineRecord;
```

### When It Runs

- **Auto-baseline**: When a fault scenario is selected, the system auto-runs a headless baseline with the same config minus faults. Stored in `BaselineStore` resource.
- **Manual**: Can be called directly for batch experiments.

## Export System (src/export/)

### CSV Export
Triggered via bridge command or desktop UI:
```
export_csv
```
Exports per-tick data: tick, alive, dead, throughput, avg_heat, cascade_depth, mttr, tasks_completed.

### JSON Export
```
export_json
```
Full simulation snapshot including config, metrics, fault events, and agent states.

## Experiment Design Patterns

### Single-Variable Comparison

Compare one variable while holding everything else constant:
```
Config A: warehouse_medium, 30 agents, pibt, random scheduler, WearBased faults
Config B: warehouse_medium, 30 agents, pibt, closest scheduler, WearBased faults
                                              ^^^^^^^ only difference
```

### Configuration Matrix

Systematically vary multiple parameters:

| Topology | Agents | Solver | Scheduler | Fault Scenario |
|----------|--------|--------|-----------|----------------|
| warehouse_medium | 8 | pibt | random | None |
| warehouse_medium | 8 | pibt | random | WearBased |
| warehouse_medium | 8 | pibt | closest | None |
| warehouse_medium | 30 | rhcr_pibt | random | IntermittentFault |
| warehouse_large | 80 | pibt | random | BurstFailure |

### Recommended Research Variables

| Variable | Values | Why |
|----------|--------|-----|
| **Scheduler** | random, closest, balanced, roundtrip | Affects task distribution; ~10% throughput effect (not dominant) |
| **Fault scenario** | None, BurstFailure, WearBased, IntermittentFault, PermanentZoneOutage | Different failure modes stress different aspects |
| **Wear intensity** | Low, Medium, High (Weibull presets) | Gradual degradation curve |
| **Topology** | warehouse_medium, warehouse_medium, warehouse_large | Scale effects |
| **Agent count** | 8, 30, 80 (match topology) | Density effects |

### What to Measure

| Metric | What it shows |
|--------|---------------|
| **Throughput delta** | Task completion rate vs fault-free baseline |
| **MTTR** | Mean Time To Recovery — how fast the system rebounds |
| **Cascade spread** | How far fault effects propagate |
| **Max TP drop** | Worst-case throughput loss during fault events |
| **Tasks completed** | Absolute productivity under faults |
| **Idle ratio** | Fraction of agents doing nothing (resource waste) |
| **Recovery index** | Scorecard metric — overall resilience rating |
| **Critical time** | Ticks spent below 50% of baseline throughput |

## Fault Scenarios

| Scenario | Description | Parameters | Best for testing |
|----------|-------------|------------|-----------------|
| None | No faults | — | Baseline reference |
| BurstFailure | Kill X% of robots at tick T | burst_fraction, burst_tick | Worst-case robustness |
| WearBased | Weibull wear model — busiest robots fail | WearHeatRate (Low/Medium/High) | Realistic degradation |
| IntermittentFault | Exponential inter-arrival temporary faults | mtbf_ticks, recovery_ticks | Transient disruption patterns |
| PermanentZoneOutage | Block cells in highest-traffic zone as permanent obstacles | perm_zone_block_percent, perm_zone_at_tick | Spatial topology disruption, forced rerouting |

### Weibull Wear Presets

| Level | Beta | Eta | ~MTTF | ~% dead at tick 500 | Literature basis |
|-------|------|-----|-------|---------------------|------------------|
| Low | 2.0 | 900 | ~800t | ~27% | CASUN AGV, well-maintained |
| Medium | 2.5 | 500 | ~445t | ~63% | Canadian survey |
| High | 3.5 | 150 | ~137t | ~90% | Carlson & Murphy 2006 |

## Solver Options

| Solver | Best for |
|--------|----------|
| `pibt` | Fast, reactive — good default for all experiments |
| `rhcr_pibt` | Windowed PIBT — better coordination, slightly more compute |
| `rhcr_pbs` | Windowed PBS — higher quality paths, slower |
| `rhcr_priority_astar` | Windowed A* — moderate density only (≤30 agents) |
| `token_passing` | Decentralized — ≤100 agents, interesting fault patterns |

## Bridge Commands for Live Experiments (WASM)

```javascript
send_command('set_topology "warehouse_medium"')
send_command('set_agents 30')
send_command('set_solver "pibt"')
send_command('set_scheduler "closest"')
send_command('set_fault_scenario {"scenario_type":"WearBased","wear_rate":"Medium"}')
send_command('start')
send_command('export_csv')
```

## Desktop Experiment UI

The desktop native build has a dedicated experiment panel (`src/ui/desktop/panels/experiment.rs`) with:
- Full-page experiment mode (takes over viewport)
- Batch configuration
- Progress tracking
- Results comparison

## Key Files

| File | Role |
|------|------|
| `src/analysis/baseline.rs` | Headless engine, BaselineConfig, BaselineRecord |
| `src/export/` | CSV/JSON export triggers and formatters |
| `src/fault/scenario.rs` | FaultScenarioType, FaultSchedule, WearHeatRate |
| `src/fault/config.rs` | FaultConfig (Weibull params, intermittent params) |
| `src/ui/bridge.rs` | Bridge commands for live configuration (WASM) |
| `src/ui/desktop/panels/experiment.rs` | Desktop experiment UI |
| `src/core/task.rs` | TaskScheduler, ActiveScheduler |
| `src/experiment/` | Experiment runner, stats, paper export |
