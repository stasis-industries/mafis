//! The live scorecard's fault tolerance must equal the experiment formula
//! applied to the same live run and its fault-free twin.

use super::common::SimHarness;
use crate::analysis::baseline::BaselineStore;
use crate::analysis::engine::AnalysisEngine;
use crate::core::grid::GridMap;
use crate::core::live_sim::LiveSim;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::{SimCommand, SimulationRunner};
use crate::core::seed::SeededRng;
use crate::core::task::ActiveScheduler;
use crate::core::topology::ZoneMap;
use crate::fault::config::{FaultConfig, FaultSource};
use crate::solver::pibt::PibtLifelongSolver;

const AGENTS: usize = 12;
const TICKS: u64 = 60;
/// Faults land no earlier than this, so both runs have warmed up.
const EARLIEST_FAULT_TICK: u64 = 20;

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

#[test]
fn observatory_ft_matches_experiment_formula() {
    let mut h = SimHarness::new(AGENTS);
    assert_eq!(h.tick(), 0);

    // Twin: the harness's own start state (same grid, agents, PIBT, seed), no faults.
    let twin_record = {
        let world = h.app.world();
        let mut twin = SimulationRunner::new(
            world.resource::<GridMap>().clone(),
            world.resource::<ZoneMap>().clone(),
            world.resource::<LiveSim>().runner.agents.clone(),
            Box::new(PibtLifelongSolver::new()),
            SeededRng::new(42),
            FaultConfig { enabled: false, ..Default::default() },
            Default::default(),
        );
        let scheduler = world.resource::<ActiveScheduler>();
        let policy = world.resource::<ActiveQueuePolicy>();
        let mut engine = AnalysisEngine::new(TICKS as usize);
        for _ in 0..TICKS {
            let mut result = twin.tick(scheduler.scheduler(), policy.policy());
            engine.record_tick(&twin, &mut result);
        }
        engine.compute_aggregates();
        engine.into_baseline_record(0, TICKS, AGENTS)
    };
    let twin_tp = twin_record.throughput_series.clone();
    h.app
        .world_mut()
        .insert_resource(BaselineStore { record: Some(twin_record), computing: false });

    // Fault on a tick where the twin delivers, so a window starting one tick
    // late would change the ratio.
    let fault_tick = (EARLIEST_FAULT_TICK..TICKS)
        .find(|&t| twin_tp[(t - 1) as usize] > 0.0)
        .expect("the twin delivers after warm-up");

    // Live run: agent 0 is killed at `fault_tick`.
    h.run_ticks(fault_tick - 1);
    h.app
        .world_mut()
        .resource_mut::<LiveSim>()
        .runner
        .enqueue_command(SimCommand::KillAgent { index: 0, source: FaultSource::Manual });
    h.run_ticks(TICKS - fault_tick + 1);

    let start = (fault_tick - 1) as usize;
    let live_tp = &h.app.world().resource::<LiveSim>().analysis.throughput_series;
    assert_eq!(live_tp.len(), TICKS as usize);
    let expected = mean(&live_tp[start..]) / mean(&twin_tp[start..]);
    let got = f64::from(h.scorecard().fault_tolerance.expect("the twin delivered after the fault"));
    assert!((got - expected).abs() < 1e-6, "scorecard {got} vs experiment formula {expected}");
}
