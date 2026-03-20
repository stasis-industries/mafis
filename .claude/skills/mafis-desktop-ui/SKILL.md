---
name: mafis-desktop-ui
description: >
  Desktop native UI guide for MAFIS (egui, non-WASM). Use this skill when working on
  the native desktop interface: egui panels, toolbar, timeline, charts, theme, keyboard
  shortcuts, or any file in src/ui/desktop/. Triggers: "add a panel", "desktop UI", "egui",
  "native UI", "add a section", "toolbar button", "keyboard shortcut", "desktop theme",
  "experiment panel", "agent list panel", "scorecard panel", "profiling panel", "chart panel",
  or any modification to src/ui/desktop/. This skill covers the egui architecture, panel
  structure, system ordering, theme, and patterns for adding new panels/sections. For web UI
  (HTML/CSS/JS in web/), use mafis-ui instead.
---

# MAFIS Desktop UI (egui)

**Applies to: `src/ui/desktop/` only (native builds, `cargo run`).** WASM builds use HTML/CSS/JS — see mafis-ui skill.

The desktop UI uses `bevy_egui` for immediate-mode GUI panels around the central Bevy 3D viewport.

## Architecture

```
src/ui/desktop/
├── mod.rs              ← DesktopUiPlugin, BridgeSet stub, system registration
├── state.rs            ← DesktopUiState resource (panel visibility, experiment mode)
├── theme.rs            ← Scientific Instrument theme for egui (apply_theme_once)
├── toolbar.rs          ← Top panel: simulation controls, view toggles
├── timeline.rs         ← Bottom panel: tick timeline visualization
├── persistence.rs      ← Save/load settings to disk
├── shortcuts.rs        ← Keyboard shortcuts (handle_shortcuts system)
├── charts/
│   ├── mod.rs          ← Chart infrastructure
│   ├── throughput.rs   ← Throughput over time
│   ├── tasks.rs        ← Tasks completed
│   └── heat.rs         ← Heat accumulation
└── panels/
    ├── mod.rs          ← left_panel_ui(), right_panel_ui() entry points
    ├── simulation.rs   ← Topology, agents, duration config
    ├── solver.rs       ← Solver selection & parameters
    ├── fault.rs        ← Manual fault injection UI
    ├── status.rs       ← Current simulation state (tick, throughput, idle)
    ├── visualization.rs← Heatmap, graphics, orbit camera controls
    ├── agent_list.rs   ← Per-agent state table + kill commands
    ├── experiment.rs   ← Batch experiment UI (fullpage mode)
    ├── export.rs       ← CSV/JSON export config
    ├── performance.rs  ← Baseline comparison stats
    ├── fault_response.rs ← Fault event timeline
    ├── profiling.rs    ← Frame time diagnostics
    └── scorecard.rs    ← Resilience metrics display
```

## Plugin Registration

```rust
pub struct DesktopUiPlugin;

impl Plugin for DesktopUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_resource::<DesktopUiState>()
            // ...
            .add_systems(EguiPrimaryContextPass, (
                theme::apply_theme_once,
                toolbar::toolbar_ui,
                timeline::timeline_ui.after(toolbar::toolbar_ui),
                panels::left_panel_ui.after(timeline::timeline_ui),
                panels::right_panel_ui.after(timeline::timeline_ui),
                experiment_fullpage_ui.after(toolbar::toolbar_ui),
            ))
            .add_systems(Update, (
                shortcuts::handle_shortcuts.in_set(BridgeSet),
                process_experiment_commands,
            ));
    }
}
```

Key: toolbar and timeline run as `TopBottomPanel`s BEFORE `SidePanel`s so they span full width.

## Panel Layout

```
┌──────────────────────────────────────────────────────────┐
│ TOOLBAR: sim controls · view toggles · settings          │
├─────────────┬──────────────────────────────┬─────────────┤
│ LEFT PANEL  │                              │ RIGHT PANEL │
│ Simulation  │     Bevy 3D Viewport         │ Status      │
│ Solver      │     (CentralPanel)           │ Scorecard   │
│ Faults      │                              │ Performance │
│ Visualization│                             │ Charts      │
│ Export      │                              │ Fault Resp  │
│             │                              │ Agents      │
│             │                              │ Profiling   │
├─────────────┴──────────────────────────────┴─────────────┤
│ TIMELINE: tick bar · fault markers · rewind controls     │
└──────────────────────────────────────────────────────────┘
```

**Experiment fullpage mode**: When `d.experiment_fullpage` is true, left/right panels and timeline are hidden. A `CentralPanel` takes over for batch experiment configuration and results.

## BridgeSet Stub

Desktop doesn't use the WASM bridge, but `FaultPlugin` references `crate::ui::BridgeSet` for system ordering. Desktop provides a stub:

```rust
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeSet;  // Stub — keyboard shortcuts run in this set
```

This is re-exported from `src/ui/mod.rs` via `#[cfg(not(target_arch = "wasm32"))]`.

## Theme (Scientific Instrument for egui)

`theme.rs` applies the Scientific Instrument aesthetic to egui:
- Cream/sand backgrounds (matching the web UI palette)
- No rounded corners on buttons/frames
- DM Mono-inspired monospace for values
- Muted separators, subtle borders
- Applied once via `apply_theme_once` system with `ThemeApplied` guard

## Adding a New Panel Section

1. Create `src/ui/desktop/panels/your_section.rs`
2. Write a function: `pub fn your_section_ui(ui: &mut egui::Ui, /* resources */) { ... }`
3. Add collapsible header in left or right panel:
```rust
// In panels/mod.rs, inside left_panel_ui or right_panel_ui
egui::CollapsingHeader::new("YOUR SECTION")
    .default_open(true)
    .show(ui, |ui| {
        your_section::your_section_ui(ui, &resources);
    });
```
4. Add `pub mod your_section;` to `panels/mod.rs`
5. Add any needed resources as system params

## Adding a Toolbar Button

In `toolbar.rs`, add to the horizontal layout:
```rust
if ui.button("YOUR ACTION").clicked() {
    // Modify resources or send commands
}
```

## Adding a Keyboard Shortcut

In `shortcuts.rs`, add to `handle_shortcuts`:
```rust
if input.just_pressed(KeyCode::KeyY) && modifiers.ctrl {
    // Your action
}
```

## Common Patterns

### Reading ECS Resources
Desktop panels have direct access to Bevy resources — no JSON serialization needed:
```rust
pub fn status_ui(ui: &mut egui::Ui, metrics: &SimMetrics, config: &SimulationConfig) {
    ui.label(format!("Tick: {}", config.tick));
    ui.label(format!("Throughput: {:.2}", metrics.throughput));
}
```

### Sending Commands
Use ECS messages or direct resource mutation:
```rust
// Via message
commands_writer.send(ManualFaultCommand::KillAgent(agent_id));

// Via resource mutation
ui_state.paused = true;
```

### Collapsible Sections
All panels use `egui::CollapsingHeader` for sections:
```rust
egui::CollapsingHeader::new("SECTION NAME")
    .default_open(false)
    .show(ui, |ui| { /* content */ });
```

## Key Differences from Web UI

| Aspect | Desktop (egui) | Web (HTML/CSS/JS) |
|--------|----------------|-------------------|
| Rendering | Immediate mode, per-frame | DOM-based, event-driven |
| Data access | Direct ECS resources | JSON via bridge polling |
| Charts | egui plot widgets | uPlot (canvas) |
| Commands | Direct resource mutation | send_command() → bridge |
| Theme | egui style API | CSS |
| Compilation | `cargo run` | WASM build pipeline |
