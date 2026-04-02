use crate::analysis::AnalysisConfig;
use crate::analysis::heatmap::{HeatmapMode, HeatmapState};
use crate::core::grid::GridMap;
use crate::render::animator::RobotOpacity;
use crate::render::graphics::GraphicsConfig;
use crate::render::orbit_camera::{self, OrbitCamera};

pub fn visualization_panel(
    ui: &mut egui::Ui,
    analysis_config: &mut AnalysisConfig,
    heatmap: &mut HeatmapState,
    graphics: &mut GraphicsConfig,
    orbit: &mut OrbitCamera,
    grid: &GridMap,
    robot_opacity: &mut RobotOpacity,
) {
    // ── Heatmap ────────────────────────────────────────────────────
    ui.checkbox(&mut analysis_config.heatmap_visible, "Show heatmap");

    if analysis_config.heatmap_visible {
        ui.indent("heatmap_indent", |ui| {
            ui.horizontal(|ui| {
                let modes = &[
                    (HeatmapMode::Density, "Density"),
                    (HeatmapMode::Traffic, "Traffic"),
                    (HeatmapMode::Criticality, "Critical"),
                ];
                for &(mode, label) in modes {
                    let selected = heatmap.mode == mode;
                    let btn = egui::Button::new(label).selected(selected);
                    if ui.add(btn).clicked() && !selected {
                        heatmap.mode = mode;
                        heatmap.dirty = true;
                    }
                }
            });

            if heatmap.mode == HeatmapMode::Density {
                ui.horizontal(|ui| {
                    ui.label("Density radius");
                    let slider = egui::Slider::new(&mut heatmap.density_radius, 1..=3);
                    if ui.add(slider).changed() {
                        heatmap.dirty = true;
                    }
                });
            }
        });
    }

    ui.add_space(4.0);

    // ── Robot opacity ──────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Robot opacity");
        ui.add(egui::Slider::new(&mut robot_opacity.opacity, 0.1..=1.0).max_decimals(2));
    });

    ui.add_space(4.0);

    // ── Camera presets ─────────────────────────────────────────────
    ui.label("Camera");
    ui.horizontal(|ui| {
        if ui.button("Top").clicked() {
            let (yaw, pitch, dist) = orbit_camera::preset_top(grid);
            orbit.target_yaw = yaw;
            orbit.target_pitch = pitch;
            orbit.target_distance = dist;
        }
        if ui.button("Side").clicked() {
            let (yaw, pitch, dist) = orbit_camera::preset_side(grid);
            orbit.target_yaw = yaw;
            orbit.target_pitch = pitch;
            orbit.target_distance = dist;
        }
    });

    ui.add_space(4.0);

    // ── Graphics ───────────────────────────────────────────────────
    ui.label("Graphics");
    ui.checkbox(&mut graphics.shadows, "Shadows");
    ui.checkbox(&mut graphics.msaa, "MSAA");
    ui.checkbox(&mut graphics.colorblind, "Colorblind Mode");
}
