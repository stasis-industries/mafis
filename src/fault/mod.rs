pub mod breakdown;
pub mod config;
pub mod heat;
pub mod manual;
pub mod scenario;

use bevy::prelude::*;

use crate::core::CoreSet;

use self::config::FaultConfig;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum FaultSet {
    Schedule,
    Heat,
    FaultCheck,
    Replan,
}

pub struct FaultPlugin;

impl Plugin for FaultPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FaultConfig>()
            .init_resource::<scenario::FaultScenario>()
            .init_resource::<scenario::FaultSchedule>()
            .init_resource::<scenario::FaultList>()
            .init_resource::<manual::ManualFaultLog>()
            .init_resource::<manual::RewindRequest>()
            .add_message::<breakdown::FaultEvent>()
            .add_message::<manual::ManualFaultCommand>()
            .configure_sets(
                FixedUpdate,
                (
                    FaultSet::Schedule.after(CoreSet::Tick),
                    FaultSet::Heat.after(FaultSet::Schedule),
                    FaultSet::FaultCheck.after(FaultSet::Heat),
                    FaultSet::Replan
                        .after(FaultSet::FaultCheck)
                        .before(CoreSet::PostTick),
                ),
            );

        // Old fault ECS systems removed — SimulationRunner handles faults
        // internally via its tick() method. Only manual fault processing
        // (user-initiated kills/latency/obstacles) remains as ECS systems.

        // Manual fault processor and replay are excluded from test builds: they
        // require Assets<Mesh> / Assets<StandardMaterial> (render assets) to spawn
        // obstacle visuals. Integration tests inject faults directly via ECS.
        #[cfg(not(test))]
        {
            use crate::core::state::SimState;
            app.add_systems(
                FixedUpdate,
                manual::replay_manual_faults
                    .in_set(FaultSet::Schedule)
                    .run_if(in_state(SimState::Running))
                    .run_if(|log: Res<manual::ManualFaultLog>| log.replay_from.is_some()),
            );
            app.add_systems(
                Update,
                (
                    manual::process_manual_faults
                        .after(crate::ui::BridgeSet),
                    manual::apply_rewind
                        .after(crate::ui::BridgeSet),
                ),
            );
        }
    }
}
