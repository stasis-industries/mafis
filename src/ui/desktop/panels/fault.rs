use bevy::prelude::*;

use crate::core::state::SimState;
use crate::fault::manual::ManualFaultCommand;
use crate::ui::controls::UiState;

use super::super::theme;

const SCENARIO_TYPES: &[(&str, &str)] = &[
    ("burst_failure", "Burst Failure"),
    ("wear_based", "Wear & Tear"),
    ("zone_outage", "Zone Outage"),
];

pub struct FaultPanelOutput {
    pub manual_cmds: Vec<ManualFaultCommand>,
}

pub fn fault_panel(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    sim_state: SimState,
    manual_x: &mut i32,
    manual_y: &mut i32,
) -> FaultPanelOutput {
    let idle = sim_state == SimState::Idle;
    let running = sim_state == SimState::Running || sim_state == SimState::Paused;
    let mut output = FaultPanelOutput { manual_cmds: Vec::new() };

    // ── Enable toggle ──────────────────────────────────────────────
    ui.horizontal(|ui| {
        let mut enabled = ui_state.fault_enabled;
        if ui.add_enabled(idle, egui::Checkbox::new(&mut enabled, "Enable fault injection")).changed() {
            ui_state.fault_enabled = enabled;
        }
    });

    if ui_state.fault_enabled {
        ui.add_space(4.0);

        // ── Scenario type ──────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Scenario");
            let current_label = SCENARIO_TYPES
                .iter()
                .find(|(id, _)| *id == ui_state.fault_scenario_type)
                .map(|(_, label)| *label)
                .unwrap_or("Unknown");

            egui::ComboBox::from_id_salt("fault_scenario")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for &(id, label) in SCENARIO_TYPES {
                        let selected = ui_state.fault_scenario_type == id;
                        if ui.selectable_label(selected, label).clicked() && !selected && idle {
                            ui_state.fault_scenario_type = id.to_string();
                        }
                    }
                });
        });

        ui.add_space(4.0);

        // ── Scenario-specific parameters ───────────────────────────
        match ui_state.fault_scenario_type.as_str() {
            "burst_failure" => {
                ui.horizontal(|ui| {
                    ui.label("Robots to kill");
                    let slider = egui::Slider::new(&mut ui_state.burst_kill_percent, 1.0..=100.0)
                        .suffix("%");
                    ui.add_enabled(idle, slider);
                });
                let abs = (ui_state.burst_kill_percent / 100.0 * ui_state.num_agents as f32).round() as usize;
                ui.weak(format!("= {} robots", abs));
                ui.horizontal(|ui| {
                    ui.label("At tick");
                    let drag = egui::DragValue::new(&mut ui_state.burst_at_tick).range(1..=5000);
                    ui.add_enabled(idle, drag);
                });
            }
            "wear_based" => {
                ui.label("Heat rate");
                ui.horizontal(|ui| {
                    let rates = &[("low", "Low"), ("medium", "Med"), ("high", "High")];
                    for &(rate, label) in rates {
                        let selected = ui_state.wear_heat_rate == rate;
                        let btn = egui::Button::new(label).selected(selected);
                        if ui.add_enabled(idle, btn).clicked() && !selected {
                            ui_state.wear_heat_rate = rate.to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Overheat threshold");
                    let slider = egui::Slider::new(&mut ui_state.wear_threshold, 10.0..=200.0);
                    ui.add_enabled(idle, slider);
                });
            }
            "zone_outage" => {
                ui.horizontal(|ui| {
                    ui.label("At tick");
                    let drag = egui::DragValue::new(&mut ui_state.zone_at_tick).range(1..=5000);
                    ui.add_enabled(idle, drag);
                });
                ui.horizontal(|ui| {
                    ui.label("Latency duration");
                    let mut d = ui_state.zone_latency_duration;
                    let slider = egui::Slider::new(&mut d, 10..=200).suffix(" ticks");
                    if ui.add_enabled(idle, slider).changed() {
                        ui_state.zone_latency_duration = d;
                    }
                });
            }
            _ => {}
        }
    }

    // ── Manual injection (visible when running/paused) ─────────────
    if running {
        ui.add_space(6.0);
        ui.separator();
        ui.colored_label(theme::TEXT_SECONDARY, "MANUAL INJECTION");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(egui::DragValue::new(manual_x).range(0..=511));
            ui.label("Y");
            ui.add(egui::DragValue::new(manual_y).range(0..=511));
        });
        if ui.button("WALL").on_hover_text("Place permanent obstacle at (X, Y)").clicked() {
            output.manual_cmds.push(ManualFaultCommand::PlaceObstacle(IVec2::new(*manual_x, *manual_y)));
        }
    }

    output
}
