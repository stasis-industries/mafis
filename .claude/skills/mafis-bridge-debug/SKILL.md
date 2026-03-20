---
name: mafis-bridge-debug
description: >
  Debug the Bevy-to-JS bridge, charts, timeline, and web UI state in MAFIS. WASM-only
  (not desktop native). Use this skill when the user reports: chart issues (stacking, wrong
  data, lines persisting, chart empty), timeline problems (markers not clearing, popup
  unclickable, DELETE broken), bridge sync issues (data not updating, stale state, numbers
  frozen), or any JS-Rust communication bug. Also trigger for: "chart is wrong", "timeline
  bug", "fault line won't go away", "can't click DELETE", "data doesn't match", "UI not
  updating", "numbers are stale", "display is wrong", "JS error", "console error", "nothing
  shows up", "dashboard is blank", "chart keeps growing", or any mismatch between what the
  simulation computes and what the browser shows. This skill knows the thread_local BRIDGE
  pattern, JS polling loop, chart data flow, timeline marker lifecycle, rewind truncation,
  and adaptive sync intervals. Not relevant for desktop/egui issues.
---

# MAFIS Bridge & UI Debugging (WASM only)

The Bevy-JS bridge is a `thread_local! { RefCell<BridgeInner> }` pattern. Rust writes JSON to an outgoing buffer, JS polls at 100ms. Commands flow in reverse: JS writes to incoming, Rust drains.

**Desktop native builds use egui directly — no bridge. This skill applies only to WASM builds.**

## Architecture

```
Rust (Bevy ECS)                          JS (web/app.js)
─────────────────                        ───────────────
sync_state_to_js (Update)                setInterval(poll, 100ms)
  → serialize ECS → JSON                   → get_simulation_state()
  → BRIDGE.outgoing = json                 → parse JSON
                                           → updateAgentTable()
process_js_commands (Update)               → updateChartData()
  ← BRIDGE.incoming.drain()               → updateTimelineBar()
  ← parse commands                         → updateFaultTimeline()
  ← apply to ECS
                                         send_command("pause")
                                           → BRIDGE.incoming.push(cmd)
```

**Key file**: `src/ui/bridge.rs`

## Diagnostic Checklist

When something is wrong in the web UI, check this sequence:

1. **Is `sync_state_to_js` running?** — Check it's not gated by wrong `SimState`
2. **Is the sync interval too slow?** — Check `bridge_sync_interval()` in constants
3. **Is data being serialized?** — Check `BridgeOutput` struct fields
4. **Is JS parsing it?** — Check `updateFromState(s)` in `app.js`
5. **Is the chart getting the right field?** — e.g. `s.metrics.throughput` vs `s.throughput`

## Common Issues & Fixes

### Chart Data Stacking After Rewind

**Symptom**: N overlapping traces after N rewinds.
**Cause**: `updateChartData()` appends without truncating at rewind point.
**Fix**: Truncate all chart arrays when `s.tick < lastChartTick`:
```javascript
if (s.tick < lastChartTick && chartData.ticks.length > 0) {
    const cutIndex = chartData.ticks.findIndex(t => t >= s.tick);
    if (cutIndex >= 0) {
        Object.values(chartData).forEach(arr => { arr.length = cutIndex; });
    }
}
```

### Fault Lines Persisting After Restart

**Symptom**: Fault markers from previous run still visible.
**Cause**: `resetChartData()` doesn't clear DOM elements.
**Fix**: In `resetChartData()`, clear timeline markers and reset counters.

### DELETE Button Unclickable

**Symptom**: Timeline popup appears but DELETE doesn't respond.
**Cause**: CSS `pointer-events: none` on `.timeline-popup`.
**Fix**: Remove `pointer-events: none` from `.timeline-popup`.

### Bridge Sync Intervals

Adaptive based on agent count (`src/constants.rs`):
| Agents | Interval |
|--------|----------|
| ≤50 | 90ms |
| 51-200 | 150ms |
| 201-400 | 500ms |
| 400+ | 1000ms |

During fast-forward (`resume_target.target_tick.is_some()`), sync is suppressed entirely.

## Chart Data Flow

```
sync_state_to_js → BridgeOutput → serde_json → BRIDGE.outgoing
                                                    ↓
get_simulation_state() → JSON string → updateFromState(s)
                                            ↓
                                      updateChartData(s)
                                        guard: same tick → skip
                                        guard: tick < last → TRUNCATE
                                        push to arrays → uPlot.setData()
```

## Timeline Marker Lifecycle

```
updateTimelineBar(s)
  → reads s.fault_schedule_markers + s.fault_events
  → needsRebuild = count changed?
  → yes: clear DOM, rebuild from scratch
  → no: just update positions
```

On rewind: fault counts stay → markers stay. On delete: Rust truncates → counts decrease → JS rebuilds.

## Browser Console Commands

```javascript
get_simulation_state()                    // raw JSON from Rust
send_command('pause')                     // control simulation
send_command('set_solver "rhcr_pibt"')    // change solver
send_command('seek_to_tick 50')           // seek
send_command('delete_fault_at_tick 100')  // delete fault event
console.log(chartData.ticks.length, lastChartTick)  // chart state
resetChartData()                          // force chart rebuild
```

## Key Files

| File | Role |
|------|------|
| `src/ui/bridge.rs` | Rust: sync_state_to_js, process_js_commands, BridgeOutput |
| `src/ui/controls.rs` | UiState resource, begin_loading, spawn agents |
| `web/app.js` | JS polling, charts, timeline, commands |
| `web/index.html` | HTML structure, 4-phase layout |
| `src/fault/manual.rs` | RewindRequest, apply_rewind, restore_world_state |
