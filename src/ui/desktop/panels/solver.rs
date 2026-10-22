use bevy::prelude::*;

use super::super::theme;
use crate::core::state::SimState;
use crate::planning::params::{ParamKind, ParamValue};
use crate::planning::registry;
use crate::ui::controls::{UiState, select_planner, set_planner_param};

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
                        select_planner(ui_state, entry.info.id.to_string());
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

    // ── Planner parameters, one slider per declared parameter ──────
    let Some(entry) = selected else {
        return;
    };
    if !entry.info.params.is_empty() {
        ui.add_space(4.0);
    }
    for spec in entry.info.params {
        let ParamKind::Int { min, max, default } = spec.kind;
        let ParamValue::Int(mut value) =
            ui_state.planner_params.get(spec.key).unwrap_or(ParamValue::Int(default));
        ui.horizontal(|ui| {
            ui.label(spec.label);
            let slider = egui::Slider::new(&mut value, min..=max);
            if ui.add_enabled(idle, slider).on_hover_text(spec.help).changed() {
                // The slider stays within the declared bounds, so this only
                // fails if the planner changed under it.
                if let Err(e) = set_planner_param(ui_state, spec.key, ParamValue::Int(value)) {
                    error!("{e}");
                }
            }
        });
    }
}
