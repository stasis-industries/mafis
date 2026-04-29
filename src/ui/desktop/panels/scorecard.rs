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
        let ft = scorecard.fault_tolerance;
        ui.colored_label(score_color(ft), format!("{ft:.2}"));
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

    // Composite score dots (0–5)
    let composite = composite_score(scorecard);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Score");
        for i in 0..5 {
            let color = if i < composite { theme::TEXT_PRIMARY } else { theme::TEXT_MUTED };
            ui.colored_label(color, "●");
        }
    });
}

/// Coarse 0–5 verdict for the desktop scorecard panel.
///
/// ⚠ NOT a research-grade metric. The implicit weights are hand-picked for
/// at-a-glance UI feedback and have no theoretical derivation. Do not cite
/// in research output — report the 4 primary metrics directly instead.
fn composite_score(sc: &ResilienceScorecard) -> usize {
    let mut score = 0.0_f32;
    score += sc.fault_tolerance.min(1.0);
    score += sc.nrr.unwrap_or(0.0);
    score += sc.survival_rate;
    score += (1.0 - sc.critical_time).max(0.0);
    ((score / 4.0) * 5.0).round() as usize
}
