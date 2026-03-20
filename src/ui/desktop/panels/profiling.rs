use bevy::prelude::*;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};

use super::super::theme;

pub fn profiling_panel(
    ui: &mut egui::Ui,
    diagnostics: &DiagnosticsStore,
) {
    if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS)
        && let Some(val) = fps.smoothed() {
            let color = if val >= 55.0 {
                theme::ZONE_GOOD
            } else if val >= 30.0 {
                theme::ZONE_FAIR
            } else {
                theme::ZONE_POOR
            };
            ui.horizontal(|ui| {
                ui.label("FPS");
                ui.colored_label(color, format!("{val:.0}"));
            });
        }

    if let Some(frame_time) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        && let Some(val) = frame_time.smoothed() {
            ui.horizontal(|ui| {
                ui.label("Frame");
                ui.monospace(format!("{val:.1} ms"));
            });
        }

    if let Some(frame_count) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_COUNT)
        && let Some(val) = frame_count.value() {
            ui.horizontal(|ui| {
                ui.label("Frames");
                ui.monospace(format!("{}", val as u64));
            });
        }
}
