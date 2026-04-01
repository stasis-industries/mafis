use std::f32::consts::{FRAC_PI_2, PI};

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::camera::ScalingMode;

#[cfg(not(target_arch = "wasm32"))]
use bevy_egui::EguiContexts;

use crate::core::grid::GridMap;

// ---------------------------------------------------------------------------
// Components & Resources
// ---------------------------------------------------------------------------

/// Marker component on the camera entity for single-entity queries.
#[derive(Component)]
pub struct OrbitCameraTag;

/// Camera projection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Perspective,
    Orthographic,
}

/// Orbital camera state in spherical coordinates around a focus point.
#[derive(Resource)]
pub struct OrbitCamera {
    /// Horizontal angle (radians). 0 = +Z axis, PI/4 = diagonal.
    pub yaw: f32,
    /// Elevation angle (radians). 0 = horizontal, PI/2 = top-down.
    pub pitch: f32,
    /// Distance from focus point.
    pub distance: f32,
    /// World-space point the camera looks at (grid center).
    pub focus: Vec3,
    /// Original grid center (for reset).
    pub default_focus: Vec3,

    // Animation targets (for smooth preset transitions)
    pub target_yaw: f32,
    pub target_pitch: f32,
    pub target_distance: f32,
    pub animating: bool,

    // 2D/3D mode
    pub mode: CameraMode,
    /// Orthographic zoom scale (world units visible vertically).
    pub ortho_scale: f32,
    pub min_ortho_scale: f32,
    pub max_ortho_scale: f32,

    // Limits
    pub min_distance: f32,
    pub max_distance: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,

    // Sensitivity
    pub orbit_sensitivity: f32,
    pub zoom_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub key_pan_speed: f32,
    pub lerp_speed: f32,
}

// ---------------------------------------------------------------------------
// Preset view calculations (pub for bridge.rs)
// ---------------------------------------------------------------------------

/// Side view: elevated diagonal (matches the original camera perspective).
pub fn preset_side(grid: &GridMap) -> (f32, f32, f32) {
    let extent = (grid.width as f32).max(grid.height as f32);
    let distance = extent * 1.28;
    let yaw = PI / 4.0;
    let pitch = 0.845; // ~48.4 degrees
    (yaw, pitch, distance)
}

/// Top view: bird's eye looking straight down.
/// Distance: Bevy default FOV ≈ 45° → visible_height ≈ 0.83 × distance.
/// extent × 1.4 gives the grid + ~15% padding.
pub fn preset_top(grid: &GridMap) -> (f32, f32, f32) {
    let extent = (grid.width as f32).max(grid.height as f32);
    let distance = extent * 1.4;
    let yaw = 0.0;
    let pitch = FRAC_PI_2 - 0.02; // ~88.8 degrees
    (yaw, pitch, distance)
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

/// Compute camera Transform from spherical coordinates around a focus point.
fn spherical_to_transform(yaw: f32, pitch: f32, distance: f32, focus: Vec3) -> Transform {
    let offset = Vec3::new(
        distance * pitch.cos() * yaw.sin(),
        distance * pitch.sin(),
        distance * pitch.cos() * yaw.cos(),
    );
    Transform::from_translation(focus + offset).looking_at(focus, Vec3::Y)
}

/// Shortest signed angular difference from `a` to `b` (result in -PI..PI).
fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (b - a) % (2.0 * PI);
    if d > PI {
        d - 2.0 * PI
    } else if d < -PI {
        d + 2.0 * PI
    } else {
        d
    }
}

/// Lerp between two angles via the shortest arc.
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + angle_diff(a, b) * t
}

/// Compute the camera-relative right and forward vectors on the XZ plane.
fn camera_planar_axes(yaw: f32) -> (Vec3, Vec3) {
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos());
    (right, forward)
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Initialize the orbit camera resource and tag the camera entity.
/// Runs after `setup_environment` so the camera entity exists.
fn setup_orbit_camera(
    mut commands: Commands,
    grid: Res<GridMap>,
    camera: Single<Entity, With<Camera3d>>,
) {
    let center_x = (grid.width as f32 - 1.0) * 0.5;
    let center_z = (grid.height as f32 - 1.0) * 0.5;
    let focus = Vec3::new(center_x, 0.0, center_z);

    let (yaw, pitch, distance) = preset_side(&grid);
    let extent = (grid.width as f32).max(grid.height as f32);

    commands.insert_resource(OrbitCamera {
        yaw,
        pitch,
        distance,
        focus,
        default_focus: focus,
        target_yaw: yaw,
        target_pitch: pitch,
        target_distance: distance,
        animating: false,
        mode: CameraMode::Perspective,
        ortho_scale: extent,
        min_ortho_scale: extent * 0.2,
        max_ortho_scale: extent * 4.0,
        min_distance: extent * 0.3,
        max_distance: extent * 3.0,
        min_pitch: 0.05,
        max_pitch: FRAC_PI_2 - 0.02,
        orbit_sensitivity: 0.003,
        zoom_sensitivity: 1.0,
        pan_sensitivity: 0.005,
        key_pan_speed: 15.0,
        lerp_speed: 6.0,
    });

    // Tag the camera entity so we can query it later
    commands.entity(*camera).insert(OrbitCameraTag);
}

/// Handle mouse drag (orbit + pan) and scroll (zoom).
pub fn orbit_mouse_input(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    accumulated_motion: Res<AccumulatedMouseMotion>,
    accumulated_scroll: Res<AccumulatedMouseScroll>,
    mut orbit: ResMut<OrbitCamera>,
    #[cfg(not(target_arch = "wasm32"))]
    mut egui_contexts: EguiContexts,
) {
    // Skip camera input when mouse is over Egui UI panels
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(ctx) = egui_contexts.ctx_mut() {
            if ctx.wants_pointer_input() || ctx.is_pointer_over_area() {
                return;
            }
        }
    }

    let delta = accumulated_motion.delta;
    let is_ortho = orbit.mode == CameraMode::Orthographic;

    // Left-mouse drag: orbit in 3D, pan in 2D
    if mouse_buttons.pressed(MouseButton::Left)
        && delta != Vec2::ZERO {
            orbit.animating = false;
            if is_ortho {
                // Pan in orthographic mode
                let pan_scale = orbit.ortho_scale * orbit.pan_sensitivity;
                orbit.focus.x -= delta.x * pan_scale;
                orbit.focus.z -= delta.y * pan_scale;
            } else {
                orbit.yaw -= delta.x * orbit.orbit_sensitivity;
                orbit.pitch = (orbit.pitch + delta.y * orbit.orbit_sensitivity)
                    .clamp(orbit.min_pitch, orbit.max_pitch);
            }
        }

    // Pan on middle or right drag (both modes)
    if (mouse_buttons.pressed(MouseButton::Middle) || mouse_buttons.pressed(MouseButton::Right))
        && delta != Vec2::ZERO {
            orbit.animating = false;
            if is_ortho {
                let pan_scale = orbit.ortho_scale * orbit.pan_sensitivity;
                orbit.focus.x -= delta.x * pan_scale;
                orbit.focus.z -= delta.y * pan_scale;
            } else {
                let pan_scale = orbit.distance * orbit.pan_sensitivity;
                let (right, forward) = camera_planar_axes(orbit.yaw);
                orbit.focus -= right * delta.x * pan_scale;
                orbit.focus += forward * delta.y * pan_scale;
            }
        }

    // Zoom on scroll
    let scroll_y = accumulated_scroll.delta.y;
    if scroll_y != 0.0 {
        orbit.animating = false;
        if is_ortho {
            orbit.ortho_scale = (orbit.ortho_scale - scroll_y * orbit.zoom_sensitivity)
                .clamp(orbit.min_ortho_scale, orbit.max_ortho_scale);
        } else {
            orbit.distance = (orbit.distance - scroll_y * orbit.zoom_sensitivity)
                .clamp(orbit.min_distance, orbit.max_distance);
        }
    }
}

/// Handle keyboard panning (WASD / ZQSD).
fn keyboard_pan_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut orbit: ResMut<OrbitCamera>,
    time: Res<Time>,
) {
    let mut pan = Vec2::ZERO;

    // WASD — Bevy uses physical key positions, so this maps to ZQSD on AZERTY
    if keys.pressed(KeyCode::KeyW) { pan.y += 1.0; }
    if keys.pressed(KeyCode::KeyS) { pan.y -= 1.0; }
    if keys.pressed(KeyCode::KeyA) { pan.x -= 1.0; }
    if keys.pressed(KeyCode::KeyD) { pan.x += 1.0; }

    if pan == Vec2::ZERO {
        return;
    }

    pan = pan.normalize();
    let speed = orbit.key_pan_speed * time.delta_secs();

    orbit.animating = false;
    if orbit.mode == CameraMode::Orthographic {
        // Direct X/Z in ortho (top-down)
        orbit.focus.x += pan.x * speed;
        orbit.focus.z -= pan.y * speed;
    } else {
        let (right, forward) = camera_planar_axes(orbit.yaw);
        orbit.focus += right * pan.x * speed;
        orbit.focus += forward * pan.y * speed;
    }
}

/// Smoothly lerp toward target values during preset transitions.
fn animate_orbit_transition(mut orbit: ResMut<OrbitCamera>, time: Res<Time>) {
    if !orbit.animating {
        return;
    }

    let t = (orbit.lerp_speed * time.delta_secs()).min(1.0);
    orbit.yaw = lerp_angle(orbit.yaw, orbit.target_yaw, t);
    orbit.pitch += (orbit.target_pitch - orbit.pitch) * t;
    orbit.distance += (orbit.target_distance - orbit.distance) * t;

    // Snap when close enough
    let eps = 0.001;
    if angle_diff(orbit.yaw, orbit.target_yaw).abs() < eps
        && (orbit.pitch - orbit.target_pitch).abs() < eps
        && (orbit.distance - orbit.target_distance).abs() < eps
    {
        orbit.yaw = orbit.target_yaw;
        orbit.pitch = orbit.target_pitch;
        orbit.distance = orbit.target_distance;
        orbit.animating = false;
    }
}

/// Write the computed transform to the camera entity each frame.
fn apply_orbit_transform(
    orbit: Res<OrbitCamera>,
    mut camera: Single<&mut Transform, With<OrbitCameraTag>>,
) {
    match orbit.mode {
        CameraMode::Perspective => {
            **camera = spherical_to_transform(orbit.yaw, orbit.pitch, orbit.distance, orbit.focus);
        }
        CameraMode::Orthographic => {
            // Top-down: camera above the focus point looking straight down
            let height = orbit.ortho_scale * 2.0;
            **camera = Transform::from_translation(
                orbit.focus + Vec3::new(0.0, height, 0.001),
            )
            .looking_at(orbit.focus, -Vec3::Z);
        }
    }
}

/// Sync camera projection component when mode changes.
fn sync_camera_projection(
    orbit: Res<OrbitCamera>,
    mut camera: Single<&mut Projection, With<OrbitCameraTag>>,
) {
    if !orbit.is_changed() {
        return;
    }

    match orbit.mode {
        CameraMode::Perspective => {
            if !matches!(**camera, Projection::Perspective(_)) {
                **camera = Projection::Perspective(PerspectiveProjection::default());
            }
        }
        CameraMode::Orthographic => {
            let ortho = OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: orbit.ortho_scale,
                },
                ..OrthographicProjection::default_3d()
            };
            match &mut **camera {
                Projection::Orthographic(p) => {
                    p.scaling_mode = ortho.scaling_mode;
                }
                _ => {
                    **camera = Projection::Orthographic(ortho);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct OrbitCameraPlugin;

impl Plugin for OrbitCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            setup_orbit_camera.after(super::environment::setup_environment),
        )
        .add_systems(
            Update,
            (
                orbit_mouse_input,
                keyboard_pan_input,
                animate_orbit_transition,
                sync_camera_projection,
                apply_orbit_transform,
            )
                .chain(),
        );
    }
}
