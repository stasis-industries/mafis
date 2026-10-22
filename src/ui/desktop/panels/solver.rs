use bevy::prelude::*;

use super::super::theme;
use crate::constants;
use crate::core::state::SimState;
use crate::planning::params::ParamValues;
use crate::planning::registry;
use crate::ui::controls::UiState;

pub fn solver_panel(ui: &mut egui::Ui, ui_state: &mut UiState, sim_state: SimState) {
    let idle = sim_state == SimState::Idle;
    let selected = registry::find(&ui_state.solver_name);

    // ── Planner dropdown ───────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Algorithm");
        let current_label = selected.map_or("Unknown", |entry| entry.info.label);

        egui::ComboBox::from_id_salt("solver_combo").selected_text(current_label).show_ui(
            ui,
            |ui| {
                for entry in registry::entries() {
                    let is_selected = ui_state.solver_name == entry.info.id;
                    if ui.selectable_label(is_selected, entry.info.label).clicked()
                        && !is_selected
                        && idle
                    {
                        ui_state.solver_name = entry.info.id.to_string();
                        ui_state.planner_params = ParamValues::default();
                        // Clear RHCR overrides when switching solver
                        ui_state.rhcr_horizon = None;
                        ui_state.rhcr_replan_interval = None;
                    }
                }
            },
        );
    });

    // ── Planner info card ──────────────────────────────────────────
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme::BG_CARD)
        .stroke(egui::Stroke::new(1.0f32, theme::BORDER))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| match selected {
            Some(entry) => {
                ui.weak(entry.info.summary);
                ui.add_space(2.0);
                ui.weak(entry.info.reference);
            }
            None => {
                ui.weak("Unknown planner.");
            }
        });

    // ── RHCR settings (only for RHCR solvers) ──────────────────────
    let is_rhcr = ui_state.solver_name.starts_with("rhcr");
    if is_rhcr {
        ui.add_space(4.0);
        ui.label("RHCR Settings");
        ui.indent("rhcr_indent", |ui| {
            // Horizon
            ui.horizontal(|ui| {
                ui.label("Horizon (H)");
                let mut h = ui_state.rhcr_horizon.unwrap_or(0) as u32;
                let auto = ui_state.rhcr_horizon.is_none();
                let slider = egui::Slider::new(
                    &mut h,
                    constants::RHCR_MIN_HORIZON as u32..=constants::RHCR_MAX_HORIZON as u32,
                )
                .suffix(if auto { " (auto)" } else { "" });
                if ui.add_enabled(idle, slider).changed() {
                    ui_state.rhcr_horizon = Some(h as usize);
                }
                if auto {
                    ui.weak("auto");
                }
            });

            // Replan interval
            ui.horizontal(|ui| {
                ui.label("Replan (W)");
                let mut w = ui_state.rhcr_replan_interval.unwrap_or(0) as u32;
                let auto = ui_state.rhcr_replan_interval.is_none();
                let slider = egui::Slider::new(
                    &mut w,
                    constants::RHCR_MIN_REPLAN_INTERVAL as u32
                        ..=constants::RHCR_MAX_REPLAN_INTERVAL as u32,
                );
                if ui.add_enabled(idle, slider).changed() {
                    ui_state.rhcr_replan_interval = Some(w as usize);
                }
                if auto {
                    ui.weak("auto");
                }
            });

            ui.weak(
                "Auto-tuned for current grid and agent count. PBS uses per-agent LRA fallback.",
            );
        });
    }
}
