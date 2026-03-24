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
        // Light theme canvas background — white (synced to dark via set_theme bridge command)
        .insert_resource(ClearColor(Color::WHITE))
        .add_plugins(MapfFisPlugin)
        .run();
}
