use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy_egui::EguiContexts;
use egui::Color32;

use crate::core::grid::GridMap;
use crate::core::state::SimState;
use crate::render::orbit_camera::{self, CameraMode, OrbitCamera};

use super::state::DesktopUiState;
use super::theme;

pub fn toolbar_ui(
    mut contexts: EguiContexts,
    sim_state: Res<State<SimState>>,
    mut orbit: ResMut<OrbitCamera>,
    grid: Res<GridMap>,
    mut desktop: ResMut<DesktopUiState>,
    diagnostics: Res<DiagnosticsStore>,
) -> Result {
    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };
    let state = **sim_state;

    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            // Title
            ui.colored_label(theme::TEXT_PRIMARY, egui::RichText::new("MAFIS").strong());
            ui.separator();

            // 2D/3D toggle
            let mode_label = match orbit.mode {
                CameraMode::Perspective => "3D",
                CameraMode::Orthographic => "2D",
            };
            if ui.button(mode_label).clicked() {
                orbit.mode = match orbit.mode {
                    CameraMode::Perspective => CameraMode::Orthographic,
                    CameraMode::Orthographic => CameraMode::Perspective,
                };
            }

            ui.separator();

            // State badge
            let (badge_text, badge_color) = match state {
                SimState::Idle => ("IDLE", theme::STATE_IDLE),
                SimState::Loading => ("LOADING", theme::STATE_PAUSED),
                SimState::Running => ("RUNNING", theme::STATE_RUNNING),
                SimState::Paused => ("PAUSED", theme::STATE_PAUSED),
                SimState::Replay => ("REPLAY", Color32::from_rgb(140, 140, 200)),
                SimState::Finished => ("FINISHED", theme::STATE_IDLE),
            };
            ui.colored_label(badge_color, badge_text);

            ui.separator();

            // EXPERIMENTS button — toggle full-page experiment mode
            let exp_label = if desktop.experiment_fullpage {
                egui::RichText::new("EXPERIMENTS").color(theme::BG_BODY).strong()
            } else {
                egui::RichText::new("EXPERIMENTS").color(theme::TEXT_SECONDARY)
            };
            let exp_btn = egui::Button::new(exp_label);
            let exp_btn = if desktop.experiment_fullpage {
                exp_btn.fill(theme::TEXT_PRIMARY)
            } else {
                exp_btn
            };
            let exp_enabled = state == SimState::Idle || desktop.experiment_fullpage;
            if ui.add_enabled(exp_enabled, exp_btn).clicked() {
                desktop.experiment_fullpage = !desktop.experiment_fullpage;
            }

            // Spacer → right-aligned items
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Settings gear (placeholder)
                ui.weak("⚙");

                // FPS counter
                let fps = diagnostics
                    .get(&FrameTimeDiagnosticsPlugin::FPS)
                    .and_then(|d| d.smoothed())
                    .unwrap_or(0.0);
                let fps_color = if fps >= 55.0 {
                    theme::ZONE_GOOD
                } else if fps >= 30.0 {
                    theme::ZONE_FAIR
                } else {
                    theme::ZONE_POOR
                };
                ui.colored_label(fps_color, format!("{:.0} FPS", fps));

                ui.separator();

                // Camera presets
                if ui.button("Side").clicked() {
                    let (y, p, d) = orbit_camera::preset_side(&grid);
                    orbit.target_yaw = y;
                    orbit.target_pitch = p;
                    orbit.target_distance = d;
                    orbit.animating = true;
                }
                if ui.button("Top").clicked() {
                    let (y, p, d) = orbit_camera::preset_top(&grid);
                    orbit.target_yaw = y;
                    orbit.target_pitch = p;
                    orbit.target_distance = d;
                    orbit.animating = true;
                }
            });
        });
    });

    Ok(())
}
