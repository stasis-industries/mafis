use std::f32::consts::PI;

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::constants;
use crate::core::action::Direction;
use crate::core::grid::GridMap;

const CELL_SIZE: f32 = 1.0;
const OBSTACLE_HEIGHT: f32 = 0.45;

pub fn grid_to_world(grid_pos: IVec2) -> Vec3 {
    Vec3::new(grid_pos.x as f32 * CELL_SIZE, 0.0, grid_pos.y as f32 * CELL_SIZE)
}

#[derive(Component)]
pub struct ObstacleMarker;

/// Marker for floor + grid line entities so they can be despawned on grid rebuild.
#[derive(Component)]
pub struct EnvironmentMarker;

/// One-time setup: camera + lights + initial floor/lines.
pub fn setup_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    grid: Res<GridMap>,
) {
    // Spawn floor + grid lines (tagged with EnvironmentMarker)
    spawn_floor_and_lines(&mut commands, &mut meshes, &mut materials, &grid);

    // Primary directional light (soft overhead)
    commands.spawn((
        DirectionalLight { illuminance: 8_000.0, shadows_enabled: false, ..default() },
        Transform::from_rotation(
            Quat::from_rotation_x(-PI / 3.0) * Quat::from_rotation_y(-PI / 6.0),
        ),
    ));

    // Rim light — secondary directional from behind/below for edge definition
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            color: Color::srgb(0.85, 0.88, 0.95),
            shadows_enabled: false,
            ..default()
        },
        Transform::from_rotation(
            Quat::from_rotation_x(PI / 3.0) * Quat::from_rotation_y(5.0 * PI / 6.0),
        ),
    ));

    let grid_width = grid.width as f32 * CELL_SIZE;
    let grid_height = grid.height as f32 * CELL_SIZE;
    let center_x = (grid.width as f32 - 1.0) * CELL_SIZE * 0.5;
    let center_z = (grid.height as f32 - 1.0) * CELL_SIZE * 0.5;

    // Camera looking at grid center from above-angle
    let cam_distance = grid_width.max(grid_height) * 1.2;
    commands.spawn((
        Camera3d::default(),
        AmbientLight { color: Color::WHITE, brightness: 450.0, affects_lightmapped_meshes: true },
        Transform::from_xyz(
            center_x + cam_distance * 0.5,
            cam_distance * 0.8,
            center_z + cam_distance * 0.5,
        )
        .looking_at(Vec3::new(center_x, 0.0, center_z), Vec3::Y),
    ));
}

/// Spawn floor plane + grid lines. Tagged with EnvironmentMarker for cleanup.
pub fn spawn_floor_and_lines(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    grid: &GridMap,
) {
    let grid_width = grid.width as f32 * CELL_SIZE;
    let grid_height = grid.height as f32 * CELL_SIZE;
    let center_x = (grid.width as f32 - 1.0) * CELL_SIZE * 0.5;
    let center_z = (grid.height as f32 - 1.0) * CELL_SIZE * 0.5;

    // Floor plane — cool lab grey
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(grid_width, grid_height))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.88, 0.88, 0.90),
            perceptual_roughness: 0.92,
            ..default()
        })),
        Transform::from_xyz(center_x, -0.01, center_z),
        EnvironmentMarker,
    ));

    // Grid lines — skip at large grids (258 entities at 128×128 = invisible noise)
    if grid.width <= constants::GRID_LINE_THRESHOLD && grid.height <= constants::GRID_LINE_THRESHOLD
    {
        let line_material = materials.add(StandardMaterial {
            base_color: Color::srgb(0.93, 0.93, 0.95),
            perceptual_roughness: 1.0,
            ..default()
        });
        let line_thickness = 0.02;
        let h_line_mesh = meshes.add(Cuboid::new(grid_width, 0.003, line_thickness));
        let v_line_mesh = meshes.add(Cuboid::new(line_thickness, 0.003, grid_height));

        for y in 0..=grid.height {
            let z = y as f32 * CELL_SIZE - CELL_SIZE * 0.5;
            commands.spawn((
                Mesh3d(h_line_mesh.clone()),
                MeshMaterial3d(line_material.clone()),
                Transform::from_xyz(center_x, 0.0, z),
                EnvironmentMarker,
            ));
        }
        for x in 0..=grid.width {
            let world_x = x as f32 * CELL_SIZE - CELL_SIZE * 0.5;
            commands.spawn((
                Mesh3d(v_line_mesh.clone()),
                MeshMaterial3d(line_material.clone()),
                Transform::from_xyz(world_x, 0.0, center_z),
                EnvironmentMarker,
            ));
        }
    }
}

/// Spawn colored floor tiles for delivery, pickup, and recharging zones.
pub fn spawn_zone_markers(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    zones: &crate::core::topology::ZoneMap,
) {
    if zones.delivery_cells.is_empty()
        && zones.pickup_cells.is_empty()
        && zones.recharging_cells.is_empty()
    {
        return;
    }

    let tile_mesh = meshes.add(Cuboid::new(CELL_SIZE * 0.95, 0.02, CELL_SIZE * 0.95));

    // Delivery: soft blue
    let delivery_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.65, 0.78, 0.88),
        perceptual_roughness: 0.9,
        ..default()
    });

    // Pickup: light warm amber
    let pickup_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.88, 0.78, 0.55),
        perceptual_roughness: 0.9,
        ..default()
    });

    // Recharging: pale cyan
    let recharging_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.68, 0.82, 0.85),
        perceptual_roughness: 0.9,
        ..default()
    });

    for &pos in &zones.delivery_cells {
        let world = grid_to_world(pos);
        commands.spawn((
            Mesh3d(tile_mesh.clone()),
            MeshMaterial3d(delivery_mat.clone()),
            Transform::from_xyz(world.x, 0.005, world.z),
            EnvironmentMarker,
        ));
    }

    for &pos in &zones.pickup_cells {
        let world = grid_to_world(pos);
        commands.spawn((
            Mesh3d(tile_mesh.clone()),
            MeshMaterial3d(pickup_mat.clone()),
            Transform::from_xyz(world.x, 0.005, world.z),
            EnvironmentMarker,
        ));
    }

    for &pos in &zones.recharging_cells {
        let world = grid_to_world(pos);
        commands.spawn((
            Mesh3d(tile_mesh.clone()),
            MeshMaterial3d(recharging_mat.clone()),
            Transform::from_xyz(world.x, 0.005, world.z),
            EnvironmentMarker,
        ));
    }
}

/// Spawn subtle direction arrows on delivery cells that have queue lines,
/// plus gradient-opacity floor tiles on the first 3 queue cells.
pub fn spawn_queue_arrows(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    zones: &crate::core::topology::ZoneMap,
) {
    if zones.queue_lines.is_empty() {
        return;
    }

    // Flat arrow mesh — a small chevron pointing in +X direction (rotated per direction)
    let arrow_mesh = meshes.add(make_arrow_mesh());

    // Subtle semi-transparent cool white with blue-tinted emissive
    let arrow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.20),
        emissive: bevy::color::LinearRgba::new(0.75, 0.78, 0.90, 1.0),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 1.0,
        unlit: true,
        ..default()
    });

    // Queue cell gradient tiles — same mesh size as zone markers
    let tile_mesh = meshes.add(Cuboid::new(CELL_SIZE * 0.95, 0.02, CELL_SIZE * 0.95));

    // Delivery zone color (soft blue) at decreasing opacities
    let queue_opacities: [f32; 3] = [0.30, 0.18, 0.08];
    let queue_mats: Vec<Handle<StandardMaterial>> = queue_opacities
        .iter()
        .map(|&alpha| {
            materials.add(StandardMaterial {
                base_color: Color::srgba(0.65, 0.78, 0.88, alpha),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.9,
                unlit: true,
                ..default()
            })
        })
        .collect();

    for line in &zones.queue_lines {
        // Arrow on the delivery cell
        let world = grid_to_world(line.delivery_cell);
        let yaw = direction_to_yaw(line.direction);
        commands.spawn((
            Mesh3d(arrow_mesh.clone()),
            MeshMaterial3d(arrow_mat.clone()),
            Transform::from_xyz(world.x, 0.015, world.z).with_rotation(Quat::from_rotation_y(yaw)),
            EnvironmentMarker,
        ));

        // Gradient floor tiles on queue cells (up to 3)
        for (i, &cell) in line.cells.iter().take(3).enumerate() {
            let cell_world = grid_to_world(cell);
            commands.spawn((
                Mesh3d(tile_mesh.clone()),
                MeshMaterial3d(queue_mats[i].clone()),
                Transform::from_xyz(cell_world.x, 0.006, cell_world.z),
                EnvironmentMarker,
            ));
        }
    }
}

/// Build a flat arrow/chevron mesh pointing in +X, lying on the XZ plane.
fn make_arrow_mesh() -> Mesh {
    // Arrow shape: a small triangle pointing right, ~0.3 cell units
    //   tip (0.2, 0)
    //   left wing (-0.1, -0.15)
    //   right wing (-0.1, 0.15)
    let positions = vec![
        [0.22, 0.0, 0.0],    // tip
        [-0.08, 0.0, -0.16], // left wing
        [-0.08, 0.0, 0.16],  // right wing
    ];
    let normals = vec![[0.0, 1.0, 0.0]; 3]; // all face up
    let uvs = vec![[0.5, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let indices = Indices::U16(vec![0, 1, 2]);

    Mesh::new(PrimitiveTopology::TriangleList, default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(indices)
}

fn direction_to_yaw(dir: Direction) -> f32 {
    match dir {
        Direction::East => 0.0,        // +X
        Direction::North => -PI / 2.0, // +Z (grid +Y maps to world +Z)
        Direction::West => PI,         // -X
        Direction::South => PI / 2.0,  // -Z
    }
}

/// Spawn obstacle cubes for all current grid obstacles.
pub fn spawn_obstacles(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    grid: &GridMap,
) {
    let obstacle_mesh = meshes.add(Cuboid::new(CELL_SIZE * 0.9, OBSTACLE_HEIGHT, CELL_SIZE * 0.9));
    let obstacle_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.47, 0.52),
        perceptual_roughness: 0.55,
        metallic: 0.15,
        ..default()
    });

    for &pos in grid.obstacles() {
        let world = grid_to_world(pos);
        commands.spawn((
            Mesh3d(obstacle_mesh.clone()),
            MeshMaterial3d(obstacle_material.clone()),
            Transform::from_xyz(world.x, OBSTACLE_HEIGHT * 0.5, world.z),
            ObstacleMarker,
        ));
    }
}

pub struct EnvironmentPlugin;

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_environment);
    }
}
