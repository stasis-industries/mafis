use bevy::prelude::*;
use std::collections::HashMap;

use crate::analysis::history::TickHistory;
use crate::core::agent::{AgentIndex, LogicalAgent};
use crate::core::state::SimState;
use crate::fault::breakdown::{Dead, LatencyFault};
use crate::fault::config::FaultConfig;
use crate::fault::heat::HeatState;

use super::environment::grid_to_world;

const LERP_SPEED: f32 = 18.0;

#[derive(Component)]
pub struct RobotVisual;


// ---------------------------------------------------------------------------
// RobotOpacity — user-controllable robot mesh transparency
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct RobotOpacity {
    pub opacity: f32,
}

impl Default for RobotOpacity {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

// ---------------------------------------------------------------------------
// MaterialPalette — pre-allocated shared materials for auto-batching
// ---------------------------------------------------------------------------

/// Number of heat-glow levels per task state.
pub const HEAT_LEVELS: usize = 4;
/// Number of task states in the 2D palette.
pub const TASK_STATES: usize = 8;

/// Base colors for each task state (sRGB) — distinct per-state palette.
pub const TASK_BASE_COLORS: [(f32, f32, f32); TASK_STATES] = [
    (0.62, 0.63, 0.67), // 0: Free — cool grey
    (0.85, 0.62, 0.15), // 1: TravelEmpty — warm amber
    (0.80, 0.42, 0.13), // 2: Loading — burnt orange
    (0.23, 0.69, 0.72), // 3: TravelToQueue — teal
    (0.16, 0.55, 0.62), // 4: Queuing — deep teal (darker, waiting)
    (0.30, 0.71, 0.46), // 5: TravelLoaded — sage green
    (0.45, 0.75, 0.30), // 6: Unloading — lime green
    (0.35, 0.53, 0.82), // 7: Charging — steel blue
];

#[derive(Resource)]
pub struct MaterialPalette {
    /// 2D palette: task_heat[state_idx][heat_level].
    /// state_idx = TaskLeg::palette_index(), heat_level = 0..HEAT_LEVELS.
    pub task_heat: [[Handle<StandardMaterial>; HEAT_LEVELS]; TASK_STATES],
    /// Dead agent: static dark red with emissive glow.
    pub dead: Handle<StandardMaterial>,
    /// Latency fault: muted purple with emissive.
    pub latency_robot: Handle<StandardMaterial>,
    /// Shared translucent goal marker material.
    pub goal_marker: Handle<StandardMaterial>,
    /// Selected robot: white with bright emissive glow.
    pub selected_robot: Handle<StandardMaterial>,
}

impl MaterialPalette {
    /// Convenience: get the cool (heat=0) material for a task state index.
    pub fn task_cool(&self, state_idx: usize) -> &Handle<StandardMaterial> {
        &self.task_heat[state_idx][0]
    }
}

fn setup_material_palette(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Build 7 × 4 task-heat palette
    let task_heat = std::array::from_fn::<_, TASK_STATES, _>(|state| {
        let (br, bg, bb) = TASK_BASE_COLORS[state];
        // Loading (2), Queuing (4), and Unloading (6) get a base emissive bump ("dwell" glow)
        let is_dwell = state == 2 || state == 4 || state == 6;
        let dwell_boost = if is_dwell { 0.8 } else { 0.0 };

        std::array::from_fn::<_, HEAT_LEVELS, _>(|heat| {
            let t = heat as f32 / (HEAT_LEVELS - 1).max(1) as f32;
            let glow = dwell_boost + t * 3.5;
            materials.add(StandardMaterial {
                base_color: Color::srgb(br, bg, bb),
                emissive: LinearRgba::new(
                    glow * br * 1.2,
                    glow * bg * 0.8,
                    glow * bb * 0.5,
                    1.0,
                ),
                perceptual_roughness: 0.35,
                metallic: 0.25,
                ..default()
            })
        })
    });

    let dead = materials.add(StandardMaterial {
        base_color: Color::srgb(0.75, 0.04, 0.04),
        emissive: LinearRgba::new(1.5, 0.0, 0.0, 1.0),
        perceptual_roughness: 0.35,
        metallic: 0.25,
        ..default()
    });

    let latency_robot = materials.add(StandardMaterial {
        base_color: Color::srgb(0.22, 0.08, 0.42),
        emissive: LinearRgba::new(0.4, 0.05, 0.8, 1.0),
        perceptual_roughness: 0.35,
        metallic: 0.25,
        ..default()
    });

    let goal_marker = materials.add(StandardMaterial {
        base_color: Color::srgba(0.2, 0.6, 0.9, 0.5),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    let selected_robot = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.94, 0.98),
        emissive: LinearRgba::new(3.8, 4.0, 4.5, 1.0),
        perceptual_roughness: 0.3,
        metallic: 0.3,
        ..default()
    });

    commands.insert_resource(MaterialPalette {
        task_heat,
        dead,
        latency_robot,
        goal_marker,
        selected_robot,
    });
}

// ---------------------------------------------------------------------------
// Robot spawn / lerp
// ---------------------------------------------------------------------------

fn spawn_robot_visuals(
    mut commands: Commands,
    agents: Query<(Entity, &LogicalAgent), Added<LogicalAgent>>,
    mut meshes: ResMut<Assets<Mesh>>,
    palette: Res<MaterialPalette>,
) {
    if agents.is_empty() {
        return;
    }

    // Flat cylinder body + thin antenna — convention for multi-agent simulators
    let body_mesh = meshes.add(Cylinder::new(0.35, 0.14));
    let antenna_mesh = meshes.add(Cylinder::new(0.02, 0.15));

    for (entity, agent) in &agents {
        let world_pos = grid_to_world(agent.current_pos);

        commands.entity(entity).with_children(|parent| {
            // Robot body (flat cylinder)
            parent.spawn((
                Mesh3d(body_mesh.clone()),
                MeshMaterial3d(palette.task_heat[0][0].clone()),
                Transform::from_xyz(0.0, 0.07, 0.0),
                RobotVisual,
            ));
            // Antenna (thin tall cylinder on top)
            parent.spawn((
                Mesh3d(antenna_mesh.clone()),
                MeshMaterial3d(palette.task_heat[0][0].clone()),
                Transform::from_xyz(0.0, 0.22, 0.0),
                RobotVisual,
            ));
        });

        commands.entity(entity).insert((
            Transform::from_xyz(world_pos.x, 0.0, world_pos.z),
            Visibility::default(),
        ));
    }
}

fn lerp_robots(
    mut agents: Query<(&LogicalAgent, &mut Transform), Without<RobotVisual>>,
    time: Res<Time>,
    step_mode: Res<crate::core::state::StepMode>,
    state: Res<State<crate::core::state::SimState>>,
) {
    // Snap instantly in step mode or when paused/replay (eliminates visual lag
    // between heatmap and robot positions when stepping one tick at a time).
    let snap = step_mode.pending
        || matches!(*state.get(), crate::core::state::SimState::Paused | crate::core::state::SimState::Replay);

    for (agent, mut transform) in &mut agents {
        let target = grid_to_world(agent.current_pos);
        let target_with_y = Vec3::new(target.x, 0.0, target.z);

        if snap {
            transform.translation = target_with_y;
        } else {
            let t = (LERP_SPEED * time.delta_secs()).min(1.0);
            transform.translation = transform.translation.lerp(target_with_y, t);
        }
    }
}

// ---------------------------------------------------------------------------
// Palette-based color update — swaps handles instead of mutating materials
// ---------------------------------------------------------------------------

fn update_robot_colors(
    agents: Query<
        (
            &Children,
            &AgentIndex,
            &LogicalAgent,
            &Transform,
            Option<&HeatState>,
            Has<Dead>,
            Has<LatencyFault>,
        ),
        With<LogicalAgent>,
    >,
    mut robot_visuals: Query<&mut MeshMaterial3d<StandardMaterial>, With<RobotVisual>>,
    palette: Res<MaterialPalette>,
    fault_config: Res<FaultConfig>,
    selection: Res<crate::render::picking::ClickSelection>,
) {
    let heat_max_idx = (HEAT_LEVELS - 1) as f32;

    for (children, agent_index, agent, _transform, heat_state, is_dead, has_latency) in &agents {
        // Priority: selected > dead > latency > task_state (always, with heat glow)
        let target_handle: &Handle<StandardMaterial> = if selection.agent_index == Some(agent_index.0) {
            &palette.selected_robot
        } else if is_dead {
            &palette.dead
        } else if has_latency {
            &palette.latency_robot
        } else {
            // Update color immediately — task state changes should be
            // visible right away, not delayed by lerp animation.

            // Task color is always primary — heat modulates glow level
            let state_idx = agent.task_leg.palette_index();
            let heat_level = if fault_config.enabled {
                if let Some(hs) = heat_state {
                    // heat is already 0-1 Weibull CDF stress indicator
                    let t = hs.heat.clamp(0.0, 1.0);
                    (t * heat_max_idx).round() as usize
                } else {
                    0
                }
            } else {
                0
            };
            let heat_level = heat_level.min(HEAT_LEVELS - 1);
            &palette.task_heat[state_idx][heat_level]
        };

        for child in children.iter() {
            // Read-only check first to avoid marking the component dirty in
            // Bevy's change detection when no material change is needed.
            // get_mut() unconditionally marks changed — skip it for the ~90%
            // of agents whose material stays the same frame-to-frame.
            let Ok(mat_handle) = robot_visuals.get(child) else {
                continue;
            };
            if mat_handle.0 != *target_handle {
                if let Ok(mut mat_handle) = robot_visuals.get_mut(child) {
                    mat_handle.0 = target_handle.clone();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Robot opacity — mutates palette materials when slider moves
// ---------------------------------------------------------------------------

fn apply_robot_opacity(
    opacity: Res<RobotOpacity>,
    palette: Res<MaterialPalette>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !opacity.is_changed() {
        return;
    }

    let alpha = opacity.opacity;
    let mode = if alpha < 1.0 {
        AlphaMode::Blend
    } else {
        AlphaMode::Opaque
    };

    // Helper closure to update a single handle
    let mut update = |handle: &Handle<StandardMaterial>| {
        if let Some(mat) = materials.get_mut(handle) {
            mat.base_color = mat.base_color.with_alpha(alpha);
            mat.alpha_mode = mode;
        }
    };

    for state in &palette.task_heat {
        for h in state {
            update(h);
        }
    }
    update(&palette.dead);
    update(&palette.latency_robot);
    update(&palette.selected_robot);
    // Note: goal_marker keeps its own transparency — not affected
}

// ---------------------------------------------------------------------------
// Replay position override — snap agents to snapshot positions
// ---------------------------------------------------------------------------

fn replay_override_positions(
    history: Res<TickHistory>,
    mut agents: Query<(&AgentIndex, &mut Transform), With<LogicalAgent>>,
    mut cached_cursor: Local<Option<usize>>,
    mut index_map: Local<HashMap<usize, IVec2>>,
) {
    let cursor = match history.replay_cursor {
        Some(c) => c,
        None => return,
    };

    // Rebuild the map only when cursor changes — O(n) once, not O(n²) every frame
    if *cached_cursor != Some(cursor) {
        *cached_cursor = Some(cursor);
        index_map.clear();
        if let Some(snapshot) = history.snapshots.get(cursor) {
            index_map.reserve(snapshot.agents.len());
            for snap in &snapshot.agents {
                index_map.insert(snap.index, snap.pos);
            }
        }
    }

    for (index, mut transform) in &mut agents {
        if let Some(&pos) = index_map.get(&index.0) {
            let world = grid_to_world(pos);
            transform.translation = Vec3::new(world.x, 0.0, world.z);
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct AnimatorPlugin;

impl Plugin for AnimatorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RobotOpacity>()
            .add_systems(Startup, setup_material_palette)
            .add_systems(
                Update,
                (
                    spawn_robot_visuals,
                    lerp_robots
                        .after(spawn_robot_visuals)
                        .run_if(not(in_state(SimState::Idle)))
                        .run_if(not(in_state(SimState::Replay))),
                    update_robot_colors
                        .after(spawn_robot_visuals)
                        .after(lerp_robots)
                        .run_if(not(in_state(SimState::Idle))),
                    apply_robot_opacity,
                    replay_override_positions
                        .after(spawn_robot_visuals)
                        .run_if(in_state(SimState::Replay)),
                ),
            );
    }
}
