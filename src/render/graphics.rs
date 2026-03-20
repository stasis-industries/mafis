use bevy::prelude::*;
use bevy::render::view::Msaa;

use crate::render::animator::{MaterialPalette, TASK_STATES, HEAT_LEVELS, TASK_BASE_COLORS};
use crate::render::orbit_camera::OrbitCameraTag;

// ---------------------------------------------------------------------------
// GraphicsConfig resource
// ---------------------------------------------------------------------------

/// Task state visual mode: controls how many distinct colors are used
/// for agent task states in the 3D view and UI legend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskStateMode {
    /// 4 macro groups: Idle, Picking, Delivering, Faulted.
    /// Best for presentations and first impressions.
    #[default]
    Simple,
    /// Full 8-state palette: every TaskLeg gets a unique color.
    /// Best for researchers drilling into queue behavior.
    Detailed,
}

#[derive(Resource, Debug, Clone)]
pub struct GraphicsConfig {
    pub shadows: bool,
    pub msaa: bool,
    pub colorblind: bool,
    pub task_state_mode: TaskStateMode,
}

impl Default for GraphicsConfig {
    fn default() -> Self {
        Self {
            shadows: false,
            msaa: true,
            colorblind: false,
            task_state_mode: TaskStateMode::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Colorblind palette — deuteranopia-safe replacements
// ---------------------------------------------------------------------------

/// Colorblind-safe base colors (deuteranopia: blue-orange-yellow, no red-green).
const COLORBLIND_TASK_COLORS: [(f32, f32, f32); TASK_STATES] = [
    (0.58, 0.60, 0.66), // 0: Free — bluer grey
    (0.88, 0.58, 0.12), // 1: TravelEmpty — bright orange
    (0.78, 0.42, 0.10), // 2: Loading — darker orange
    (0.60, 0.75, 0.85), // 3: TravelToQueue — light blue
    (0.45, 0.60, 0.78), // 4: Queuing — muted blue (darker, waiting)
    (0.25, 0.50, 0.82), // 5: TravelLoaded — blue (replaces green)
    (0.30, 0.55, 0.90), // 6: Unloading — brighter blue
    (0.82, 0.78, 0.18), // 7: Charging — yellow
];

/// Simple mode: 4 macro groups mapped onto 8 palette slots.
///
/// | Macro Group  | TaskLeg slots                              | Color       |
/// |--------------|--------------------------------------------|-------------|
/// | **Idle**     | 0: Free, 7: Charging                       | cool grey   |
/// | **Picking**  | 1: TravelEmpty, 2: Loading                 | warm amber  |
/// | **Delivering**| 3: TravelToQueue, 4: Queuing, 5: TravelLoaded, 6: Unloading | teal |
const SIMPLE_TASK_COLORS: [(f32, f32, f32); TASK_STATES] = [
    (0.62, 0.63, 0.67), // 0: Free        → Idle (grey)
    (0.85, 0.62, 0.15), // 1: TravelEmpty → Picking (amber)
    (0.85, 0.62, 0.15), // 2: Loading     → Picking (amber)
    (0.23, 0.69, 0.72), // 3: TravelToQueue → Delivering (teal)
    (0.23, 0.69, 0.72), // 4: Queuing     → Delivering (teal)
    (0.23, 0.69, 0.72), // 5: TravelLoaded → Delivering (teal)
    (0.23, 0.69, 0.72), // 6: Unloading   → Delivering (teal)
    (0.62, 0.63, 0.67), // 7: Charging    → Idle (grey)
];

/// Simple mode + colorblind: same macro grouping, deuteranopia-safe colors.
const SIMPLE_COLORBLIND_TASK_COLORS: [(f32, f32, f32); TASK_STATES] = [
    (0.58, 0.60, 0.66), // 0: Free        → Idle (bluer grey)
    (0.88, 0.58, 0.12), // 1: TravelEmpty → Picking (bright orange)
    (0.88, 0.58, 0.12), // 2: Loading     → Picking (bright orange)
    (0.45, 0.60, 0.82), // 3: TravelToQueue → Delivering (blue)
    (0.45, 0.60, 0.82), // 4: Queuing     → Delivering (blue)
    (0.45, 0.60, 0.82), // 5: TravelLoaded → Delivering (blue)
    (0.45, 0.60, 0.82), // 6: Unloading   → Delivering (blue)
    (0.58, 0.60, 0.66), // 7: Charging    → Idle (bluer grey)
];

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Toggle directional light shadows when config changes.
pub fn apply_shadows(
    config: Res<GraphicsConfig>,
    mut lights: Query<&mut DirectionalLight>,
) {
    if !config.is_changed() {
        return;
    }
    for mut light in &mut lights {
        if light.shadows_enabled != config.shadows {
            light.shadows_enabled = config.shadows;
        }
    }
}

/// Toggle MSAA on the camera when config changes.
pub fn apply_msaa(
    config: Res<GraphicsConfig>,
    mut commands: Commands,
    camera: Query<Entity, With<OrbitCameraTag>>,
) {
    if !config.is_changed() {
        return;
    }
    let msaa = if config.msaa { Msaa::Sample4 } else { Msaa::Off };
    for entity in &camera {
        commands.entity(entity).insert(msaa);
    }
}

/// Rebuild task-heat palette materials when colorblind or task state mode changes.
///
/// Selects from 4 palettes: {Simple, Detailed} × {Normal, Colorblind}.
/// In Simple mode, multiple TaskLeg states share the same color (macro groups).
/// In Detailed mode, every state gets a unique color.
pub fn apply_visual_palette(
    config: Res<GraphicsConfig>,
    palette: Res<MaterialPalette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !config.is_changed() {
        return;
    }

    let colors = match (config.task_state_mode, config.colorblind) {
        (TaskStateMode::Detailed, false) => &TASK_BASE_COLORS,
        (TaskStateMode::Detailed, true)  => &COLORBLIND_TASK_COLORS,
        (TaskStateMode::Simple,   false) => &SIMPLE_TASK_COLORS,
        (TaskStateMode::Simple,   true)  => &SIMPLE_COLORBLIND_TASK_COLORS,
    };

    // Update the 2D task-heat palette (8 states × 4 heat levels)
    for (state, &(br, bg, bb)) in colors.iter().enumerate().take(TASK_STATES) {
        // Dwell glow only in detailed mode (Loading=2, Queuing=4, Unloading=6)
        let is_dwell = config.task_state_mode == TaskStateMode::Detailed
            && (state == 2 || state == 4 || state == 6);
        let dwell_boost = if is_dwell { 0.8 } else { 0.0 };

        for heat in 0..HEAT_LEVELS {
            let t = heat as f32 / (HEAT_LEVELS - 1).max(1) as f32;
            let glow = dwell_boost + t * 3.5;
            if let Some(mat) = materials.get_mut(&palette.task_heat[state][heat]) {
                mat.base_color = Color::srgb(br, bg, bb);
                mat.emissive = LinearRgba::new(
                    glow * br * 1.2,
                    glow * bg * 0.8,
                    glow * bb * 0.5,
                    1.0,
                );
            }
        }
    }

    // Update latency + dead
    if let Some(mat) = materials.get_mut(&palette.latency_robot) {
        if config.colorblind {
            mat.base_color = Color::srgb(0.82, 0.78, 0.18);
            mat.emissive = LinearRgba::new(2.0, 1.8, 0.2, 1.0);
        } else {
            mat.base_color = Color::srgb(0.50, 0.35, 0.72);
            mat.emissive = LinearRgba::new(1.5, 0.3, 2.0, 1.0);
        }
    }

    if let Some(mat) = materials.get_mut(&palette.dead) {
        if config.colorblind {
            mat.base_color = Color::srgb(0.25, 0.10, 0.10);
            mat.emissive = LinearRgba::new(5.0, 0.5, 0.0, 1.0);
        } else {
            mat.base_color = Color::srgb(0.90, 0.17, 0.01);
            mat.emissive = LinearRgba::new(3.0, 0.3, 0.0, 1.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct GraphicsPlugin;

impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphicsConfig>()
            .add_systems(
                Update,
                (apply_shadows, apply_msaa, apply_visual_palette),
            );
    }
}
