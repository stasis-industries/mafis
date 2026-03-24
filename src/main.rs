use bevy::prelude::*;
use mafis::MapfFisPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "MAFIS — Multi-Agent Fault Injection Simulator".into(),
                #[cfg(target_arch = "wasm32")]
                canvas: Some("#bevy-canvas".into()),
                #[cfg(target_arch = "wasm32")]
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        // Light theme canvas background — matches --bg-canvas: rgb(225, 222, 218)
        .insert_resource(ClearColor(Color::srgb(0.882, 0.871, 0.855)))
        .add_plugins(MapfFisPlugin)
        .run();
}
