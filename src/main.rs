use bevy::prelude::*;
use mafis::MapfFisPlugin;

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            #[cfg(feature = "headless")]
            title: "MAFIS — Experiment Runner".into(),
            #[cfg(not(feature = "headless"))]
            title: "MAFIS — Multi-Agent Fault Injection Simulator".into(),
            #[cfg(target_arch = "wasm32")]
            canvas: Some("#bevy-canvas".into()),
            #[cfg(target_arch = "wasm32")]
            fit_canvas_to_parent: true,
            ..default()
        }),
        ..default()
    }));

    // Dark background for headless (egui dark theme), white for observatory
    #[cfg(feature = "headless")]
    app.insert_resource(ClearColor(Color::srgb(18.0 / 255.0, 18.0 / 255.0, 22.0 / 255.0)));
    #[cfg(not(feature = "headless"))]
    app.insert_resource(ClearColor(Color::WHITE));

    app.add_plugins(MapfFisPlugin).run();
}
