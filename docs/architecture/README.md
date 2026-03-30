# Architecture

How MAFIS is structured — from the Bevy ECS schedule to the JavaScript UI.

---

## Overview

MAFIS is a Bevy 0.18 application compiled to WebAssembly. The simulation runs in Rust (ECS), and the UI runs in JavaScript. They communicate through a thread-local bridge that serializes state to JSON.

```
┌──────────────────────────────────────────┐
│              Bevy ECS (Rust/WASM)         │
│                                          │
│  FixedUpdate:  Simulation + Analysis     │
│  Update:       Rendering + Bridge sync   │
└─────────────────┬────────────────────────┘
                  │ JSON via thread-local BRIDGE
┌─────────────────▼────────────────────────┐
│           JavaScript / Web UI            │
│                                          │
│  Polls at 100ms, sends commands back     │
│  uPlot charts, HTML controls             │
└──────────────────────────────────────────┘
```

---

## Plugin Structure

The application is composed of plugins, registered in this order:

```mermaid
graph TD
    A[MapfFisPlugin] --> B[CorePlugin]
    A --> C[SolverPlugin]
    A --> D[FaultPlugin]
    A --> E[AnalysisPlugin]
    A --> F[UiPlugin]
    F --> F1[ControlsPlugin]
    F --> F2[BridgePlugin]
    A --> G[RenderPlugin]
    A --> H[ExportPlugin]

    style G stroke-dasharray: 5 5
    style H stroke-dasharray: 5 5
```

Dashed = conditional (only in Observatory/WASM builds, not headless).

---

## Tick Execution

Every fixed-timestep tick, the simulation advances through 10 phases in strict order. This all happens inside `SimulationRunner::tick()`.

```mermaid
flowchart TD
    P0["Phase 0: Increment tick counter"]
    P1["Phase 1: Process queued commands\n(kill agent, place obstacle, inject latency)"]
    P2["Phase 2: Execute fault schedule\n(timed scenario events)"]
    P3["Phase 3: Apply latency faults\n(frozen agents skip movement)"]
    P4["Phase 4: Collision resolution\n+ position update"]
    P5["Phase 5: Task state machine\n(recycle_goals)"]
    P55["Phase 5.5: Queue manager\n(arrivals, compaction, promotion)"]
    P6["Phase 6: Solver step\n(pathfinding for agents needing plans)"]
    P7["Phase 7: Fault pipeline\n(heat accumulation + fault detection)"]
    P8["Phase 8: Replan agents whose\npaths cross new obstacles"]
    P9["Phase 9: Build TickResult\n(return fault events)"]

    P0 --> P1 --> P2 --> P3 --> P4 --> P5 --> P55 --> P6 --> P7 --> P8 --> P9
```

All phases see the same tick number (incremented first in Phase 0).

---

## ECS Schedule

The Bevy schedule has two layers: **FixedUpdate** (simulation) and **Update** (rendering + bridge).

```mermaid
flowchart TD
    subgraph FixedUpdate ["FixedUpdate (at tick_hz)"]
        direction TB
        subgraph CoreTick ["CoreSet::Tick"]
            DS["drive_simulation\n(runs SimulationRunner::tick)"]
            SR["sync_runner_to_ecs\n(writes positions, heat, tasks to ECS)"]
            DS --> SR
        end
        subgraph PostTick ["CoreSet::PostTick"]
            direction TB
            subgraph Analysis ["AnalysisSet"]
                BG["BuildGraph\n(build ADG)"]
                CA["Cascade\n(propagate faults)"]
                ME["Metrics\n(AET, MTTR, scorecard,\nheatmap, tick snapshot)"]
                BG --> CA --> ME
            end
        end
        CoreTick --> PostTick
    end

    subgraph Update ["Update (every frame)"]
        direction TB
        SYNC["sync_state_to_js\n(ECS → JSON → BRIDGE)"]
        PROC["process_js_commands\n(BRIDGE → ECS mutations)"]
        REN["Rendering systems\n(lerp, colors, orbit camera,\nheatmap, picking)"]
    end

    FixedUpdate ~~~ Update
```

**FixedUpdate** runs at the configured `tick_hz` (1-30 Hz). **Update** runs every frame (typically 60 fps). The rendering systems interpolate between ticks for smooth visuals.

### Run conditions

| System | Runs when |
|--------|-----------|
| `drive_simulation` | `SimState::Running` and `LiveSim` resource exists |
| `build_adg` | Any cascade or fault metric is enabled, or heatmap criticality mode |
| `propagate_cascade` | Any cascade or fault metric is enabled |
| `update_metrics` | Any core metric is enabled |
| `sync_state_to_js` | Always (WASM only), but adaptive interval throttles frequency |

---

## Bridge Data Flow

The bridge is the communication layer between Rust and JavaScript. It uses a `thread_local` (safe in single-threaded WASM).

```mermaid
sequenceDiagram
    participant ECS as Bevy ECS
    participant Bridge as thread_local BRIDGE
    participant JS as JavaScript UI

    loop Every frame (adaptive interval)
        ECS->>ECS: Query agents, metrics, config
        ECS->>Bridge: Write BridgeOutput to BRIDGE.outgoing
    end

    loop Every 100ms
        JS->>Bridge: get_simulation_state()
        Bridge-->>JS: JSON snapshot
        JS->>JS: Update charts, controls, status
    end

    JS->>Bridge: send_command(json)
    Bridge->>Bridge: Parse → push to BRIDGE.incoming

    loop Every frame
        ECS->>Bridge: Drain BRIDGE.incoming
        Bridge-->>ECS: Vec<JsCommand>
        ECS->>ECS: Apply commands to ECS
    end
```

### Adaptive sync interval

The bridge doesn't serialize every frame — it throttles based on agent count to avoid JSON overhead:

| Agent count | Sync interval |
|-------------|---------------|
| 1-50 | 90ms |
| 51-200 | 150ms |
| 201-400 | 500ms |
| 400+ | 1000ms |

Above the **aggregate threshold** (50 agents), the bridge sends an `AgentSummary` (counts, average heat, histogram) instead of individual agent snapshots.

### What flows through the bridge

**Rust -> JS (outgoing):**
- Simulation state (tick, duration, tick_hz, state)
- Agent data (positions, goals, heat, task legs — or summary above threshold)
- Metrics (AET, makespan, MTTR, fault counts, cascade depth, scorecard)
- Fault events (last 100)
- Configuration state
- Heatmap mode and visibility
- Task leg distribution counts

**JS -> Rust (incoming commands):**
- Simulation control: start, pause, resume, reset, step
- Configuration: num_agents, seed, tick_hz, solver, topology
- Fault injection: kill agent, place obstacle, inject latency
- Analysis toggles: heatmap mode, metric on/off
- Export triggers
- Camera/graphics presets

---

## Simulation States

The simulation itself has a state machine controlling its lifecycle:

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Loading : Start
    Loading --> Running : Baseline + agents ready
    Running --> Paused : Pause
    Paused --> Running : Resume / Step
    Running --> Finished : Duration reached
    Paused --> Finished : Duration reached
    Finished --> Replay : Enter replay
    Paused --> Replay : Enter replay
    Replay --> Replay : Seek / Step
    Idle --> Idle : Reset (from any state)
    Running --> Idle : Reset
    Paused --> Idle : Reset
    Finished --> Idle : Reset
    Replay --> Idle : Reset
```

**Loading** runs the fault-free baseline simulation, spawns agents, and computes the initial solve. This happens in the background so the UI stays responsive.

---

## Source Layout

```
src/
  main.rs          Entry point (creates App, configures window)
  lib.rs           MapfFisPlugin + wasm_bindgen exports
  constants.rs     All tunable limits (MAX_AGENTS, ADG_AGENT_LIMIT, etc.)
  core/            Tick loop, agents, grid, state machine, task scheduling, topology
  solver/          8 lifelong solvers + shared heuristics + A*
  fault/           Heat/wear accumulation, fault detection, replanning
  analysis/        ADG, cascade BFS, metrics, heatmap, scorecard
  render/          Environment, robot visuals, orbit camera, picking/hover
  ui/              controls.rs (UiState), bridge.rs (Bevy-JS bridge)
  export/          CSV/JSON export with triggers
web/
  index.html       HTML/CSS shell
  app.js           JS polling loop, uPlot charts, bridge commands
```
