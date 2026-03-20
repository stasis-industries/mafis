---
name: mafis-perf
description: >
  Performance diagnosis and optimization guide for MAFIS. Use this skill when the user
  reports performance issues: "slow", "lag", "FPS drop", "frame time", "bottleneck", "why is
  it slow", "optimize", "performance regression", "choppy", "stuttering", or asks about WASM
  overhead, GPU batching, Bevy rendering costs, or egui performance. Also trigger when the user
  is about to add a system that runs every tick and wants to know performance implications, or
  after any change that touches FixedUpdate systems, heatmap rendering, or agent count scaling.
  This skill contains the known performance profile, proven optimizations, and common traps
  specific to this Bevy 0.18 project (both WASM and native desktop).
---

# MAFIS Performance Guide

## Platform Limits

| Platform | MAX_AGENTS | Loading batch | Notes |
|----------|-----------|---------------|-------|
| WASM (browser) | 1,000 | 100/frame | Single-threaded, linear memory |
| Native desktop | 5,000 | 1,000/frame | Multi-threaded Bevy executor |

## Current Performance Profile

Measured on release WASM build, 500 agents, 128x128 grid, faults active:

| Configuration | FPS |
|---------------|-----|
| Heatmap OFF | 100-180 |
| Heatmap ON | 65-94 |
| Previous (before opts) | 5-20 |

Target: **60+ FPS** at max config. Anything below 60 is a regression.

## The Big Wins (already applied)

These optimizations gave 10-30x improvement. Reverting any would be catastrophic:

1. **Shadows disabled** (`shadows_enabled: false`) — eliminates extra render pass
2. **Opaque heatmap tiles** — was `AlphaMode::Blend` (forces separate pass), now opaque
3. **No WASM logging** — `console.log` via wasm-bindgen is extremely expensive per-call
4. **MaterialPalette handle swap** — color changes = swap `Handle<StandardMaterial>`, never `materials.get_mut()`. Enables GPU batching.
5. **Unchained Update systems** — render systems run in parallel, not sequentially
6. **Grid lines skip** above `GRID_LINE_THRESHOLD` (64) — avoids thousands of line entities

## Performance-Critical Patterns

### MaterialPalette (src/render/animator.rs)

Pre-allocated shared material handles. Robots change color by swapping their `MeshMaterial3d<StandardMaterial>` handle — never mutating the asset.

```rust
// CORRECT — handle swap, GPU batching preserved
commands.entity(e).insert(MeshMaterial3d(palette.idle_robot.clone()));

// WRONG — breaks GPU batching, forces asset reload
let mat = materials.get_mut(&handle).unwrap();
mat.base_color = Color::RED;
```

The `update_robot_colors` system uses handle comparison (`current != target`) before swapping — avoids unnecessary change detection.

### Heatmap (src/analysis/heatmap.rs)

- Opaque tiles only — `AlphaMode::Blend` requires depth sorting (O(n log n) per frame)
- Tiles use `Visibility::Hidden/Visible` toggle, not spawn/despawn
- Density threshold (`DENSITY_MIN_THRESHOLD=1.8`) filters single-robot cells

### Bridge Sync — WASM only (src/ui/bridge.rs)

Adaptive sync interval prevents serialization overhead from dominating:
| Agents | Interval | Why |
|--------|----------|-----|
| ≤50 | 90ms | Small JSON, frequent updates OK |
| 51-200 | 150ms | Medium JSON |
| 201-400 | 500ms | Large JSON, serialize less often |
| 400+ | 1000ms | Huge JSON, minimize serialization |

Above `AGGREGATE_THRESHOLD` (50 agents): sends `AgentSummary` instead of per-agent snapshots.
During fast-forward: sync suppressed entirely.

### ADG Stride (src/analysis/dependency.rs)

ADG computation scales with agent count — uses stride to skip ticks:
| Agents | Stride | Betweenness |
|--------|--------|-------------|
| ≤100 | every tick | every 50 ticks |
| 101-300 | every 3 ticks | every 50 ticks |
| 301-500 | every 5 ticks | every 50 ticks |
| 500+ | every 8 ticks | disabled (>200 agents) |

### FixedUpdate vs Update

- **FixedUpdate**: Simulation logic (tick, faults, analysis). Runs at fixed rate regardless of frame time. Adding expensive work here directly impacts tick rate.
- **Update**: Rendering, UI sync, interpolation. Runs per-frame. Adding expensive work here impacts FPS.

Rule: never do O(n²) work in FixedUpdate. If you need per-agent-pair computation, use spatial indexing or run it less frequently.

## Common Performance Traps

| Trap | Cost | Fix |
|------|------|-----|
| `materials.get_mut()` on shared handle | Breaks batching for ALL users of that material | Use MaterialPalette handle swap |
| `AlphaMode::Blend` on many entities | O(n log n) depth sort per frame | Use opaque + Visibility toggle |
| `console.log` / `web_sys::console` in hot path | ~0.1ms per call via WASM bridge | Remove or gate behind `#[cfg(debug_assertions)]` |
| `HashMap` in per-tick system | Allocation + hashing overhead | Use `Vec` with index or pre-allocated map |
| Spawning/despawning entities every tick | Archetype fragmentation | Spawn once, toggle Visibility |
| `Query<&mut T>` when only reading | Triggers change detection write | Use `Query<&T>` (immutable) |
| Large `serde_json::to_string` every frame | Serialization dominates frame | Use adaptive sync interval |
| Grid lines on large grids | Thousands of line entities | Skip above GRID_LINE_THRESHOLD |
| ADG on high agent counts | O(n²) graph construction | Use ADG stride / disable above limit |

## Desktop-Specific (egui)

- egui runs in `EguiPrimaryContextPass` — not in `Update`. Keep panel code lightweight.
- Avoid `egui::plot` with thousands of data points per frame — use chart stride.
- Frame time diagnostics via `FrameTimeDiagnosticsPlugin` (already registered).
- Native gets true multi-threading from Bevy's parallel executor — less sensitive to per-system cost.

## Profiling Checklist

When diagnosing a performance issue:

1. **Confirm it's a regression** — compare against known baseline (see profile above)
2. **Is it FixedUpdate or Update?** — If tick rate drops, it's FixedUpdate. If FPS drops but ticks are fine, it's Update.
3. **Check agent count** — Performance is usually O(n) in agents. 1000 (WASM) / 5000 (native) is the stress test.
4. **Check heatmap** — Toggle off. If FPS jumps, the heatmap system is the bottleneck.
5. **Check for `get_mut` calls** — Search for `materials.get_mut` or `meshes.get_mut` in changed code.
6. **Check for new spawns** — Any system spawning entities per-tick?
7. **Check serialization (WASM)** — Is `sync_state_to_js` taking longer? (larger BridgeOutput struct)
8. **Check ADG stride** — Is ADG running too frequently for the agent count?

## WASM-Specific Considerations

- **No threads** — Everything is single-threaded in WASM. Bevy's parallel executor still helps via task scheduling, but true parallelism is unavailable.
- **Memory** — WASM linear memory grows but never shrinks. Pre-allocate buffers, reuse `Vec`s.
- **getrandom** — Must use `wasm_js` feature. System entropy calls are slower than native.
- **String serialization** — Crossing the WASM-JS boundary is expensive. Minimize `String` traffic.

## Solver Performance Reference (native release)

| Scenario | PIBT | PIBT-Window | Priority A* | PBS |
|----------|------|-------------|-------------|-----|
| Small WH (8 agents) | - | 22µs | 422µs | 741µs |
| Medium WH (30 agents) | - | 143µs | 4455µs | 2186µs |

PIBT-Window is the fastest windowed planner. PBS has higher per-replan cost but better path quality. Priority A* scales poorly above 30 agents.

## Files to Profile

| File | Hot path | What to watch |
|------|----------|---------------|
| `src/render/animator.rs` | `update_robot_colors` | Handle swaps per frame |
| `src/analysis/heatmap.rs` | `update_heatmap_*` | Tile updates per frame |
| `src/ui/bridge.rs` | `sync_state_to_js` | Serialization cost (WASM) |
| `src/solver/pibt_core.rs` | `one_step` | Per-tick solver cost |
| `src/core/runner.rs` | `SimulationRunner::tick` | Movement + collision + faults |
| `src/analysis/dependency.rs` | ADG systems | Disabled above limit |
