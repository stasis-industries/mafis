use bevy::prelude::*;

use crate::constants;
use crate::core::state::{SimState, SimulationConfig};
use crate::core::task::{ActiveScheduler, SCHEDULER_NAMES};
use crate::core::topology::{ActiveTopology, CustomMap, TopologyRegistry};
use crate::ui::controls::UiState;

const DURATION_PRESETS: &[(&str, u64)] = &[
    ("Short", 200),
    ("Medium", 500),
    ("Long", 1000),
];

pub fn simulation_panel(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    config: &mut SimulationConfig,
    sim_state: SimState,
    scheduler: &mut ActiveScheduler,
    topology: &mut ActiveTopology,
    registry: &TopologyRegistry,
) {
    let idle = sim_state == SimState::Idle;

    // ── Topology (MAP) — loaded from topologies/ ───────────────────
    ui.label("Map");
    if registry.entries.is_empty() {
        ui.weak("No maps found in topologies/");
    }

    // Auto-apply: if topology_name matches a registry entry but ActiveTopology
    // still holds a built-in preset (not a CustomMap), load the JSON data now.
    // This happens on startup when the default topology_name matches an entry ID.
    if topology.name() != "custom" {
        if let Some(entry) = registry.entries.iter().find(|e| e.id == ui_state.topology_name) {
            if let Some((grid, zones)) = TopologyRegistry::parse_entry(entry) {
                ui_state.grid_width = grid.width;
                ui_state.grid_height = grid.height;
                ui_state.num_agents = entry.suggested_agents;
                topology.set(Box::new(CustomMap { grid, zones }));
            }
        }
    }

    // Import map from file
    if ui.add_enabled(idle, egui::Button::new("Import Map…")).clicked() {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Import Map JSON")
            .add_filter("JSON", &["json"])
            .pick_file()
        {
            if let Ok(json_data) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_data) {
                    if let Some((grid, zones)) = TopologyRegistry::parse_json_value(&v) {
                        let id = path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("imported")
                            .replace('-', "_");

                        ui_state.topology_name = id;
                        ui_state.grid_width = grid.width;
                        ui_state.grid_height = grid.height;

                        // Use suggested_agents from JSON, or count robots, or keep current
                        if let Some(n) = v.get("suggested_agents").and_then(|v| v.as_u64()) {
                            ui_state.num_agents = n as usize;
                        } else if let Some(robots) = v.get("robots").and_then(|r| r.as_array()) {
                            if !robots.is_empty() {
                                ui_state.num_agents = robots.len();
                            }
                        }

                        topology.set(Box::new(CustomMap { grid, zones }));
                    }
                }
            }
        }
    }

    ui.add_space(4.0);

    for entry in &registry.entries {
        let selected = ui_state.topology_name == entry.id;
        let desc = format!("{}×{} · {} agents", entry.width, entry.height, entry.suggested_agents);
        let text = if selected {
            egui::RichText::new(format!("▸ {}  {}", entry.name, desc)).strong()
        } else {
            egui::RichText::new(format!("  {}  {}", entry.name, desc))
        };
        let btn = egui::Button::new(text)
            .selected(selected)
            .min_size(egui::vec2(ui.available_width(), 0.0));
        if ui.add_enabled(idle, btn).clicked() && !selected {
            // Parse JSON and apply as custom map
            if let Some((grid, zones)) = TopologyRegistry::parse_entry(entry) {
                ui_state.topology_name = entry.id.clone();
                ui_state.grid_width = grid.width;
                ui_state.grid_height = grid.height;
                ui_state.num_agents = entry.suggested_agents;
                topology.set(Box::new(CustomMap { grid, zones }));
            }
        }
    }

    ui.add_space(6.0);

    // ── Agents ─────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Agents");
        let mut n = ui_state.num_agents as u32;
        let slider = egui::Slider::new(
            &mut n,
            constants::MIN_AGENTS as u32..=constants::MAX_AGENTS as u32,
        );
        if ui.add_enabled(idle, slider).changed() {
            ui_state.num_agents = n as usize;
        }
    });

    // ── Seed ───────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Seed");
        let mut s = ui_state.seed as u32;
        let drag = egui::DragValue::new(&mut s).range(0..=9999);
        if ui.add_enabled(idle, drag).changed() {
            ui_state.seed = s as u64;
        }
    });

    ui.add_space(4.0);

    // ── Scheduler ──────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Scheduler");
        let current = scheduler.name().to_string();
        egui::ComboBox::from_id_salt("scheduler_combo")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for &(id, label) in SCHEDULER_NAMES {
                    let is_selected = current == id;
                    if ui.selectable_label(is_selected, label).clicked() && !is_selected && idle {
                        *scheduler = ActiveScheduler::from_name(id);
                    }
                }
            });
    });

    ui.add_space(4.0);

    // ── Duration ───────────────────────────────────────────────────
    ui.label("Duration");
    ui.horizontal(|ui| {
        for &(label, val) in DURATION_PRESETS {
            let selected = config.duration == val;
            let btn = egui::Button::new(label).selected(selected);
            if ui.add_enabled(idle, btn).clicked() {
                config.duration = val;
            }
        }
    });
    ui.horizontal(|ui| {
        let mut d = config.duration as u32;
        let drag = egui::DragValue::new(&mut d)
            .range(constants::MIN_DURATION as u32..=constants::MAX_DURATION as u32)
            .suffix(" ticks");
        if ui.add_enabled(idle, drag).changed() {
            config.duration = d as u64;
        }
    });
}
