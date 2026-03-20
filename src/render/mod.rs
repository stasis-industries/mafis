pub mod animator;
pub mod environment;
pub mod graphics;
pub mod orbit_camera;
pub mod picking;

use bevy::prelude::*;

pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            environment::EnvironmentPlugin,
            animator::AnimatorPlugin,
            orbit_camera::OrbitCameraPlugin,
            graphics::GraphicsPlugin,
            picking::PickingPlugin,
        ));
    }
}
