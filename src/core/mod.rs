pub mod action;
pub mod agent;
pub mod grid;
pub mod live_sim;
pub mod phase;
pub mod placement;
pub mod queue;
pub mod runner;
pub mod seed;
pub mod simulation;
pub mod state;
pub mod task;
pub mod topology;

use bevy::prelude::*;

use self::live_sim::LiveSim;
use self::state::SimState;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoreSet {
    Tick,
    PostTick,
}

pub struct CorePlugin;

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(FixedUpdate, (CoreSet::Tick, CoreSet::PostTick).chain())
            .add_plugins((
                state::StatePlugin,
                grid::GridPlugin,
                agent::AgentPlugin,
                seed::SeedPlugin,
                simulation::SimulationPlugin,
                task::TaskPlugin,
                queue::QueuePlugin,
                phase::PhasePlugin,
                topology::TopologyPlugin,
            ))
            // ── Runner-driven tick chain ──────────────────────────────
            .add_systems(
                FixedUpdate,
                (live_sim::drive_simulation, live_sim::sync_runner_to_ecs)
                    .chain()
                    .in_set(CoreSet::Tick)
                    .run_if(in_state(SimState::Running))
                    .run_if(resource_exists::<LiveSim>),
            );
    }
}
