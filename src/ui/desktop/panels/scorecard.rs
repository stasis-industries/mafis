use egui::Color32;

use super::super::theme;
use crate::analysis::scorecard::ResilienceScorecard;

fn score_color(val: f32) -> Color32 {
    if val >= 0.7 {
        theme::ZONE_GOOD
    } else if val >= 0.4 {
        theme::ZONE_FAIR
    } else {
        theme::ZONE_POOR
    }
}

pub fn scorecard_panel(ui: &mut egui::Ui, scorecard: &ResilienceScorecard) {
    if !scorecard.has_faults {
        ui.weak("No faults — scorecard inactive");
        return;
    }

    egui::Grid::new("scorecard_grid").show(ui, |ui| {
        // Fault Tolerance
        ui.label("Fault Tolerance");
        match scorecard.fault_tolerance {
            Some(ft) => {
                ui.colored_label(score_color(ft), format!("{ft:.2}"));
            }
            None => {
                ui.weak("N/A (the twin completed no task after the fault)");
            }
        }
        ui.end_row();

        // NRR
        ui.label("NRR");
        match scorecard.nrr {
            Some(nrr) => {
                ui.colored_label(score_color(nrr), format!("{nrr:.2}"));
            }
            None => {
                ui.weak("N/A (requires 2+ fault events)");
            }
        }
        ui.end_row();

        // Survival Rate
        ui.label("Survival Rate");
        let a = scorecard.survival_rate;
        ui.colored_label(score_color(a), format!("{a:.2}"));
        ui.end_row();

        // Critical Time
        ui.label("Critical Time");
        let ct = scorecard.critical_time;
        let ct_color = if ct <= 0.2 {
            theme::ZONE_GOOD
        } else if ct <= 0.5 {
            theme::ZONE_FAIR
        } else {
            theme::ZONE_POOR
        };
        ui.colored_label(ct_color, format!("{ct:.2}"));
        ui.end_row();
    });
}
