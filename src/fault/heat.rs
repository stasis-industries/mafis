use bevy::prelude::*;

#[derive(Component, Debug, Default)]
pub struct HeatState {
    pub heat: f32,
    pub total_moves: u32,
}

// Old accumulate_heat ECS system removed —
// SimulationRunner handles heat accumulation internally.
