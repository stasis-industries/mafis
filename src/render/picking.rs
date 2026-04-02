use bevy::prelude::*;

use crate::core::agent::{AgentIndex, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::state::SimState;
use crate::render::environment::grid_to_world;
use crate::render::orbit_camera::OrbitCameraTag;

// ---------------------------------------------------------------------------
// ClickSelection resource
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct ClickSelection {
    pub agent_index: Option<usize>,
    pub cell: Option<IVec2>,
    pub screen_x: f32,
    pub screen_y: f32,
    /// Set to true when a new click is detected; bridge reads + clears.
    pub fresh: bool,
}

// ---------------------------------------------------------------------------
// HoverHighlight — tile under cursor
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct HoverHighlight {
    pub cell: Option<IVec2>,
}

/// Marker for the single hover-highlight tile entity.
#[derive(Component)]
pub struct HoverTile;

// ---------------------------------------------------------------------------
// Ray-plane intersection (pure function, testable)
// ---------------------------------------------------------------------------

/// Intersect a ray with the Y=0 plane. Returns the hit point or None if
/// the ray is parallel to / pointing away from the plane.
pub fn ray_floor_intersection(origin: Vec3, direction: Vec3) -> Option<Vec3> {
    if direction.y.abs() < 1e-6 {
        return None; // parallel
    }
    let t = -origin.y / direction.y;
    if t < 0.0 {
        return None; // behind camera
    }
    Some(origin + direction * t)
}

/// Convert a world-space floor hit to the nearest grid cell.
pub fn world_to_grid_cell(hit: Vec3) -> IVec2 {
    IVec2::new(hit.x.round() as i32, hit.z.round() as i32)
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

/// Detect viewport clicks (not orbit drags) and write to ClickSelection.
///
/// Runs in Update, after orbit_mouse_input so we can distinguish click vs drag.
/// A click is a press+release where the cursor moved less than 5 px.
fn detect_viewport_click(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<OrbitCameraTag>>,
    agents: Query<(&LogicalAgent, &AgentIndex, &Transform)>,
    grid: Res<GridMap>,
    sim_state: Res<State<SimState>>,
    mut selection: ResMut<ClickSelection>,
    mut press_pos: Local<Option<Vec2>>,
) {
    let current = *sim_state.get();
    if current == SimState::Idle || current == SimState::Loading {
        return;
    }

    // Record press position
    if mouse_buttons.just_pressed(MouseButton::Left)
        && let Ok(window) = windows.single()
    {
        *press_pos = window.cursor_position();
    }

    // Check release
    if mouse_buttons.just_released(MouseButton::Left) {
        let Some(start) = press_pos.take() else {
            return;
        };
        let Ok(window) = windows.single() else {
            return;
        };
        let Some(cursor) = window.cursor_position() else {
            return;
        };

        // Drag discrimination: > 5 px = orbit drag, not a click
        if start.distance(cursor) > 5.0 {
            return;
        }

        let Ok((camera, cam_transform)) = camera_q.single() else {
            return;
        };

        // Raycast from cursor through camera
        let Some(ray) = camera.viewport_to_world(cam_transform, cursor).ok() else {
            return;
        };

        let Some(hit) = ray_floor_intersection(ray.origin, *ray.direction) else {
            return;
        };

        let cell = world_to_grid_cell(hit);

        if !grid.is_in_bounds(cell) {
            return;
        }

        // Check if any agent occupies this cell — match on both logical
        // position (current_pos) AND visual position (Transform) so that
        // mid-lerp agents are still selectable where they visually appear.
        let mut found_agent: Option<usize> = None;
        for (agent, index, transform) in &agents {
            if agent.current_pos == cell {
                found_agent = Some(index.0);
                break;
            }
            let visual_cell = world_to_grid_cell(transform.translation);
            if visual_cell == cell {
                found_agent = Some(index.0);
                break;
            }
        }

        selection.agent_index = found_agent;
        selection.cell = Some(cell);
        selection.screen_x = cursor.x;
        selection.screen_y = cursor.y;
        selection.fresh = true;
    }
}

// ---------------------------------------------------------------------------
// Hover highlight system
// ---------------------------------------------------------------------------

fn update_hover_highlight(
    windows: Query<&Window>,
    camera_q: Query<(&Camera, &GlobalTransform), With<OrbitCameraTag>>,
    grid: Res<GridMap>,
    sim_state: Res<State<SimState>>,
    mut hover: ResMut<HoverHighlight>,
    mut tile_q: Query<(&mut Transform, &mut Visibility), With<HoverTile>>,
) {
    let current = *sim_state.get();
    if current == SimState::Idle || current == SimState::Loading {
        // Hide tile when not in sim
        hover.cell = None;
        for (_, mut vis) in &mut tile_q {
            *vis = Visibility::Hidden;
        }
        return;
    }

    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        hover.cell = None;
        for (_, mut vis) in &mut tile_q {
            *vis = Visibility::Hidden;
        }
        return;
    };

    let Ok((camera, cam_transform)) = camera_q.single() else {
        return;
    };

    let Some(ray) = camera.viewport_to_world(cam_transform, cursor).ok() else {
        return;
    };

    let Some(hit) = ray_floor_intersection(ray.origin, *ray.direction) else {
        hover.cell = None;
        for (_, mut vis) in &mut tile_q {
            *vis = Visibility::Hidden;
        }
        return;
    };

    let cell = world_to_grid_cell(hit);

    if !grid.is_in_bounds(cell) {
        hover.cell = None;
        for (_, mut vis) in &mut tile_q {
            *vis = Visibility::Hidden;
        }
        return;
    }

    hover.cell = Some(cell);
    let world = grid_to_world(cell);

    for (mut transform, mut vis) in &mut tile_q {
        transform.translation = Vec3::new(world.x, 0.008, world.z);
        *vis = Visibility::Visible;
    }
}

fn spawn_hover_tile(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Cuboid::new(0.95, 0.005, 0.95));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.15),
        emissive: LinearRgba::new(0.7, 0.75, 0.95, 1.0),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, -10.0, 0.0), // off-screen initially
        Visibility::Hidden,
        HoverTile,
    ));
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct PickingPlugin;

impl Plugin for PickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClickSelection>()
            .init_resource::<HoverHighlight>()
            .add_systems(Startup, spawn_hover_tile)
            .add_systems(
                Update,
                (
                    detect_viewport_click.after(super::orbit_camera::orbit_mouse_input),
                    update_hover_highlight.after(super::orbit_camera::orbit_mouse_input),
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_floor_from_above() {
        let origin = Vec3::new(5.0, 10.0, 5.0);
        let direction = Vec3::new(0.0, -1.0, 0.0);
        let hit = ray_floor_intersection(origin, direction).unwrap();
        assert!((hit.x - 5.0).abs() < 1e-4);
        assert!(hit.y.abs() < 1e-4);
        assert!((hit.z - 5.0).abs() < 1e-4);
    }

    #[test]
    fn ray_hits_floor_at_angle() {
        // Camera at (0, 10, 0) looking diagonally down toward (5, 0, 5)
        let origin = Vec3::new(0.0, 10.0, 0.0);
        let direction = Vec3::new(5.0, -10.0, 5.0).normalize();
        let hit = ray_floor_intersection(origin, direction).unwrap();
        assert!((hit.x - 5.0).abs() < 1e-3);
        assert!(hit.y.abs() < 1e-3);
        assert!((hit.z - 5.0).abs() < 1e-3);
    }

    #[test]
    fn ray_parallel_to_floor_returns_none() {
        let origin = Vec3::new(0.0, 5.0, 0.0);
        let direction = Vec3::new(1.0, 0.0, 0.0);
        assert!(ray_floor_intersection(origin, direction).is_none());
    }

    #[test]
    fn ray_pointing_away_returns_none() {
        let origin = Vec3::new(0.0, 5.0, 0.0);
        let direction = Vec3::new(0.0, 1.0, 0.0); // pointing up
        assert!(ray_floor_intersection(origin, direction).is_none());
    }

    #[test]
    fn world_to_grid_rounding() {
        // Exact cell center
        assert_eq!(world_to_grid_cell(Vec3::new(3.0, 0.0, 7.0)), IVec2::new(3, 7));
        // Slightly off-center rounds correctly
        assert_eq!(world_to_grid_cell(Vec3::new(3.3, 0.0, 7.7)), IVec2::new(3, 8));
        assert_eq!(world_to_grid_cell(Vec3::new(3.5, 0.0, 7.5)), IVec2::new(4, 8));
        // Negative coords
        assert_eq!(world_to_grid_cell(Vec3::new(-0.3, 0.0, -0.3)), IVec2::new(0, 0));
    }
}
