use bevy::prelude::*;

#[derive(Component, Debug)]
pub struct HeatState {
    pub heat: f32,
    pub total_moves: u32,
}

impl Default for HeatState {
    fn default() -> Self {
        Self { heat: 0.0, total_moves: 0 }
    }
}

// Old accumulate_heat ECS system removed —
// SimulationRunner handles heat accumulation internally.
