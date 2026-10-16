//! Diagnostic probe (ignored): baseline health of PIBT / RHCR-PBS per
//! (topology, density), 10 seeds, 500 ticks, closest scheduler, no faults.
//! Run: MAFIS_PROBE_SOLVERS=rhcr_pbs,pibt cargo test --release --test solver_fidelity_probe -- --ignored --nocapture

use mafis::analysis::baseline::place_agents;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::ActiveTopology;
use mafis::fault::config::FaultConfig;
use mafis::fault::scenario::FaultSchedule;
use rayon::prelude::*;
use std::time::Instant;

const SEEDS: &[u64] = &[42, 123, 456, 789, 1024, 2048, 3141, 9999, 1337, 7777];
const TICKS: u64 = 500;
const STALL_WINDOW: usize = 50;

fn run(solver: &str, topo: &str, n: usize, seed: u64) -> (f64, bool, f64, Option<f32>, f64, f32) {
    let t = ActiveTopology::from_name(topo);
    let out = t.topology().generate(seed);
    let (grid, zones) = (out.grid, out.zones);
    let area = (grid.width * grid.height) as usize;
    let mut rng = SeededRng::new(seed);
    let agents = place_agents(n, &grid, &zones, &mut rng);
    let s = mafis::solver::lifelong_solver_from_name(solver, area, n).unwrap();
    let mut runner = SimulationRunner::new(
        grid,
        zones,
        agents,
        s,
        rng,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );
    let sched = ActiveScheduler::from_name("closest");
    let qp = ActiveQueuePolicy::from_name("closest");
    let mut per_tick = Vec::with_capacity(TICKS as usize);
    let mut prev = 0u64;
    let start = Instant::now();
    for _ in 0..TICKS {
        runner.tick(sched.scheduler(), qp.policy());
        per_tick.push(runner.tasks_completed - prev);
        prev = runner.tasks_completed;
    }
    let us_per_tick = start.elapsed().as_micros() as f64 / TICKS as f64;
    let stalled = per_tick.windows(STALL_WINDOW).any(|w| w.iter().all(|&c| c == 0));
    let s = runner.solver();
    (
        prev as f64 / TICKS as f64,
        stalled,
        us_per_tick,
        s.pbs_partial_rate(),
        s.extra_replan_count().unwrap_or(0) as f64,
        s.congested_window_rate().unwrap_or(f32::NAN),
    )
}

#[test]
#[ignore]
fn solver_fidelity_probe() {
    let solvers = std::env::var("MAFIS_PROBE_SOLVERS").unwrap_or_else(|_| "rhcr_pbs,pibt".into());
    let cells: &[(&str, usize)] = &[
        ("warehouse_single_dock", 20),
        ("warehouse_single_dock", 40),
        ("warehouse_single_dock", 60),
        ("warehouse_dual_dock", 40),
        ("warehouse_dual_dock", 80),
        ("warehouse_dual_dock", 120),
    ];
    println!(
        "solver,topology,n,mean_tp,min_tp,max_tp,stalled_seeds,mean_us_per_tick,mean_partial,mean_extra_replans,mean_congested"
    );
    for solver in solvers.split(',') {
        for &(topo, n) in cells {
            let rs: Vec<_> = SEEDS.par_iter().map(|&s| run(solver, topo, n, s)).collect();
            let tps: Vec<f64> = rs.iter().map(|r| r.0).collect();
            let mean = tps.iter().sum::<f64>() / tps.len() as f64;
            let min = tps.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = tps.iter().cloned().fold(0.0, f64::max);
            let stalled = rs.iter().filter(|r| r.1).count();
            let us = rs.iter().map(|r| r.2).sum::<f64>() / rs.len() as f64;
            let pr: Vec<f32> = rs.iter().filter_map(|r| r.3).collect();
            let pr_mean =
                if pr.is_empty() { f32::NAN } else { pr.iter().sum::<f32>() / pr.len() as f32 };
            let extra = rs.iter().map(|r| r.4).sum::<f64>() / rs.len() as f64;
            let cong = rs.iter().map(|r| r.5).sum::<f32>() / rs.len() as f32;
            println!(
                "{solver},{topo},{n},{mean:.3},{min:.3},{max:.3},{stalled},{us:.0},{pr_mean:.3},{extra:.0},{cong:.3}"
            );
        }
    }
}
