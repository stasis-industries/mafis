//! Diagnostic probe (ignored): baseline health of each registered planner (or
//! the ids in MAFIS_PROBE_SOLVERS) per (topology, density), 10 seeds, 500
//! ticks, closest scheduler, no faults, with the mean of each telemetry key.
//! Run: cargo test --release --test solver_fidelity_probe -- --ignored --nocapture

use mafis::analysis::baseline::place_agents;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::ActiveTopology;
use mafis::fault::config::FaultConfig;
use mafis::fault::scenario::FaultSchedule;
use mafis::planning::planner::Telemetry;
use mafis::planning::registry;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::time::Instant;

const SEEDS: &[u64] = &[42, 123, 456, 789, 1024, 2048, 3141, 9999, 1337, 7777];
const TICKS: u64 = 500;
const STALL_WINDOW: usize = 50;

fn run(planner: &str, topo: &str, n: usize, seed: u64) -> (f64, bool, f64, Telemetry) {
    let t = ActiveTopology::from_name(topo);
    let out = t.topology().generate(seed);
    let (grid, zones) = (out.grid, out.zones);
    let mut rng = SeededRng::new(seed);
    let agents = place_agents(n, &grid, &zones, &mut rng);
    let s = registry::build(planner, &Default::default(), n).unwrap_or_else(|e| panic!("{e}"));
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
    (prev as f64 / TICKS as f64, stalled, us_per_tick, runner.planner().telemetry())
}

#[test]
#[ignore]
fn solver_fidelity_probe() {
    let planners: Vec<String> = match std::env::var("MAFIS_PROBE_SOLVERS") {
        Ok(ids) => ids.split(',').map(str::to_string).collect(),
        Err(_) => registry::entries().iter().map(|entry| entry.info.id.to_string()).collect(),
    };
    let cells: &[(&str, usize)] = &[
        ("warehouse_single_dock", 20),
        ("warehouse_single_dock", 40),
        ("warehouse_single_dock", 60),
        ("warehouse_dual_dock", 40),
        ("warehouse_dual_dock", 80),
        ("warehouse_dual_dock", 120),
    ];
    println!(
        "planner,topology,n,mean_tp,min_tp,max_tp,stalled_seeds,mean_us_per_tick,telemetry_means"
    );
    for planner in &planners {
        for &(topo, n) in cells {
            let rs: Vec<_> = SEEDS.par_iter().map(|&s| run(planner, topo, n, s)).collect();
            let tps: Vec<f64> = rs.iter().map(|r| r.0).collect();
            let mean = tps.iter().sum::<f64>() / tps.len() as f64;
            let min = tps.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = tps.iter().cloned().fold(0.0, f64::max);
            let stalled = rs.iter().filter(|r| r.1).count();
            let us = rs.iter().map(|r| r.2).sum::<f64>() / rs.len() as f64;
            // Mean of each key over the seeds that report it.
            let mut sums: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
            for (key, value) in rs.iter().flat_map(|r| r.3.iter()) {
                let (sum, count) = sums.entry(key.as_str()).or_default();
                *sum += value;
                *count += 1;
            }
            let telemetry: Vec<String> = sums
                .iter()
                .map(|(key, (sum, count))| format!("{key}={:.3}", sum / *count as f64))
                .collect();
            println!(
                "{planner},{topo},{n},{mean:.3},{min:.3},{max:.3},{stalled},{us:.0},{}",
                telemetry.join(";")
            );
        }
    }
}
