use bevy::prelude::*;
use bevy_egui::input::EguiWantsInput;

use crate::analysis::AnalysisConfig;
use crate::core::grid::GridMap;
use crate::core::state::{SimState, SimulationConfig, StepMode};
use crate::render::orbit_camera::{self, CameraMode, OrbitCamera};

pub fn handle_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    egui_input: Res<EguiWantsInput>,
    sim_state: Res<State<SimState>>,
    mut next_state: ResMut<NextState<SimState>>,
    mut step_mode: ResMut<StepMode>,
    mut config: ResMut<SimulationConfig>,
    mut orbit: ResMut<OrbitCamera>,
    mut analysis_config: ResMut<AnalysisConfig>,
    grid: Res<GridMap>,
) {
    // Don't process shortcuts when egui has keyboard focus (text input, etc.)
    if egui_input.wants_any_keyboard_input() {
        return;
    }

    let state = **sim_state;

    // Space — play/pause toggle
    if keys.just_pressed(KeyCode::Space) {
        match state {
            SimState::Idle => next_state.set(SimState::Loading),
            SimState::Running => next_state.set(SimState::Paused),
            SimState::Paused => next_state.set(SimState::Running),
            _ => {}
        }
    }

    // N — step forward (when paused)
    if keys.just_pressed(KeyCode::KeyN) && state == SimState::Paused {
        step_mode.pending = true;
        next_state.set(SimState::Running);
    }

    // R — reset
    if keys.just_pressed(KeyCode::KeyR) && state != SimState::Idle && state != SimState::Loading {
        next_state.set(SimState::Idle);
    }

    // 1 — top camera preset
    if keys.just_pressed(KeyCode::Digit1) {
        let (y, p, d) = orbit_camera::preset_top(&grid);
        orbit.target_yaw = y;
        orbit.target_pitch = p;
        orbit.target_distance = d;
        orbit.animating = true;
    }

    // 2 — side camera preset
    if keys.just_pressed(KeyCode::Digit2) {
        let (y, p, d) = orbit_camera::preset_side(&grid);
        orbit.target_yaw = y;
        orbit.target_pitch = p;
        orbit.target_distance = d;
        orbit.animating = true;
    }

    // H — toggle heatmap
    if keys.just_pressed(KeyCode::KeyH) {
        analysis_config.heatmap_visible = !analysis_config.heatmap_visible;
    }

    // M — toggle 2D/3D camera mode
    if keys.just_pressed(KeyCode::KeyM) {
        orbit.mode = match orbit.mode {
            CameraMode::Perspective => CameraMode::Orthographic,
            CameraMode::Orthographic => CameraMode::Perspective,
        };
    }

    // +/- — adjust tick speed
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        config.tick_hz = (config.tick_hz + 2.0).min(30.0);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        config.tick_hz = (config.tick_hz - 2.0).max(1.0);
    }
}
