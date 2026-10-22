//! Test helpers shared by the planner folders.

use bevy::math::IVec2;

use crate::analysis::baseline::place_agents;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::SimulationRunner;
use crate::core::seed::SeededRng;
use crate::core::task::ActiveScheduler;
use crate::core::topology::ActiveTopology;
use crate::fault::config::FaultConfig;
use crate::fault::scenario::FaultSchedule;
use crate::planning::planner::Planner;

/// A fault-free run on `warehouse_single_dock` with 12 agents and seed 42.
pub fn small_run(planner: Box<dyn Planner>) -> SimulationRunner {
    let output = ActiveTopology::from_name("warehouse_single_dock").topology().generate(42);
    let mut rng = SeededRng::new(42);
    let agents = place_agents(12, &output.grid, &output.zones, &mut rng);
    SimulationRunner::new(
        output.grid,
        output.zones,
        agents,
        planner,
        rng,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    )
}

/// One tick with the random scheduler and the closest-queue policy.
pub fn tick(runner: &mut SimulationRunner) {
    let scheduler = ActiveScheduler::from_name("random");
    let policy = ActiveQueuePolicy::from_name("closest");
    runner.tick(scheduler.scheduler(), policy.policy());
}

/// The first tick after `cut`, up to `end`, at which a run whose planner was
/// reset and restored from its own snapshot at `cut` has its agents somewhere
/// else than the same run left alone. `None` when the two never part.
pub fn first_divergence_after_restore(
    make: fn() -> Box<dyn Planner>,
    cut: u64,
    end: u64,
) -> Option<u64> {
    let positions =
        |runner: &SimulationRunner| -> Vec<IVec2> { runner.agents.iter().map(|a| a.pos).collect() };
    let mut plain = small_run(make());
    let mut restored = small_run(make());
    for _ in 0..cut {
        tick(&mut plain);
        tick(&mut restored);
    }
    let snapshot = restored.planner().snapshot().expect("the planner supports snapshots");
    restored.planner_mut().reset();
    restored.planner_mut().restore(&snapshot);
    (cut + 1..=end).find(|_| {
        tick(&mut plain);
        tick(&mut restored);
        positions(&plain) != positions(&restored)
    })
}
