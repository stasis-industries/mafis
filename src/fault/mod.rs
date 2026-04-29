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
            .init_resource::<manual::PendingManualFaults>()
            .add_message::<breakdown::FaultEvent>()
            .add_message::<manual::ManualFaultCommand>()
            .configure_sets(FixedUpdate, FaultSet::Schedule.after(CoreSet::Tick));

        // Old fault ECS systems removed — SimulationRunner handles faults
        // internally via its tick() method. Only manual fault processing
        // (user-initiated kills/latency/obstacles) remains as ECS systems.

        // Manual fault processor and replay are excluded from test and headless
        // builds: they require Assets<Mesh> / Assets<StandardMaterial> (render assets)
        // to spawn obstacle visuals. Integration tests inject faults directly via ECS.
        #[cfg(not(any(test, feature = "headless")))]
        {
            use crate::core::state::SimState;
            app.add_systems(
                FixedUpdate,
                (
                    manual::replay_manual_faults
                        .in_set(FaultSet::Schedule)
                        .run_if(in_state(SimState::Running))
                        .run_if(|log: Res<manual::ManualFaultLog>| log.replay_from.is_some()),
                    // Drains buffered manual fault triggers (collected in `Update`)
                    // and emits `FaultEvent`s in `FixedUpdate` so the `propagate_cascade`
                    // reader picks them up on the same schedule it reads scheduled faults.
                    manual::drain_pending_manual_faults
                        .in_set(FaultSet::Schedule)
                        .after(manual::replay_manual_faults)
                        .run_if(in_state(SimState::Running)),
                ),
            );
            app.add_systems(
                Update,
                (
                    manual::process_manual_faults.after(crate::ui::BridgeSet),
                    manual::apply_rewind.after(crate::ui::BridgeSet),
                ),
            );
        }
    }
}
