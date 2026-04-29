//! Phase 0 reliability trace: single-seed investigation of Rapidity = 0 on
//! PIBT/Intermittent. Prints per-tick throughput + smoothed ratio so the
//! audit team can decide whether the metric reading is genuine instant absorb
//! or a smoothing-window edge case.
//!
//! Run: `cargo test --release --test trace_rapidity trace_rapidity_pibt_intermittent -- --ignored --nocapture`

use mafis::analysis::baseline::place_agents;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::ActiveTopology;
use mafis::fault::scenario::{FaultScenario, FaultScenarioType, FaultSchedule, WearHeatRate};
use std::fs::{self, File};
use std::io::Write;

const TICK_COUNT: u64 = 500;
const SMOOTH_WINDOW: usize = 20; // RAPIDITY_SMOOTH_WINDOW
const RAPIDITY_THRESHOLD: f64 = 0.9;
const RAPIDITY_DWELL: usize = 5;

fn rolling_average(series: &[f64], window: usize) -> Vec<f64> {
    if series.is_empty() || window == 0 {
        return series.to_vec();
    }
    let w = window.min(series.len());
    let mut out = Vec::with_capacity(series.len());
    let mut sum = 0.0_f64;
    for (i, &v) in series.iter().enumerate() {
        sum += v;
        if i >= w {
            sum -= series[i - w];
            out.push(sum / w as f64);
        } else {
            out.push(sum / (i + 1) as f64);
        }
    }
    out
}

fn per_tick_throughput(tasks_cumulative: &[u64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(tasks_cumulative.len());
    let mut prev = 0_u64;
    for &cum in tasks_cumulative {
        out.push((cum.saturating_sub(prev)) as f64);
        prev = cum;
    }
    out
}

fn make_scenario(name: &str) -> FaultScenario {
    match name {
        "burst_20pct" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            ..Default::default()
        },
        "burst_50pct" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 50.0,
            burst_at_tick: 100,
            ..Default::default()
        },
        "wear_medium" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::WearBased,
            wear_heat_rate: WearHeatRate::Medium,
            wear_threshold: 80.0,
            ..Default::default()
        },
        "wear_high" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::WearBased,
            wear_heat_rate: WearHeatRate::High,
            wear_threshold: 60.0,
            ..Default::default()
        },
        "zone_50t" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::ZoneOutage,
            zone_at_tick: 100,
            zone_latency_duration: 50,
            ..Default::default()
        },
        "intermittent_80s80m15r" => FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::IntermittentFault,
            intermittent_mtbf_ticks: 80,
            intermittent_recovery_ticks: 15,
            intermittent_start_tick: 80,
            ..Default::default()
        },
        _ => panic!("unknown scenario: {name}"),
    }
}

fn run_trace(
    solver_name: &str,
    scenario_name: &str,
    topo_name: &str,
    num_agents: usize,
    seed: u64,
) {
    let active = ActiveTopology::from_name(topo_name);
    let output = active.topology().generate(seed);
    let grid = output.grid;
    let zones = output.zones;
    let grid_area = (grid.width * grid.height) as usize;

    let scheduler = ActiveScheduler::from_name("closest");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    let mut rng = SeededRng::new(seed);
    let agents = place_agents(num_agents, &grid, &zones, &mut rng);
    let rng_after_placement = rng.clone();

    // Baseline run (no faults)
    let baseline_tp = {
        let solver =
            mafis::solver::lifelong_solver_from_name(solver_name, grid_area, num_agents).unwrap();
        let fault_config =
            mafis::fault::config::FaultConfig { enabled: false, ..Default::default() };
        let mut runner = SimulationRunner::new(
            grid.clone(),
            zones.clone(),
            agents.clone(),
            solver,
            rng_after_placement.clone(),
            fault_config,
            FaultSchedule::default(),
        );
        let mut tasks_cum = Vec::with_capacity(TICK_COUNT as usize);
        for _ in 0..TICK_COUNT {
            runner.tick(scheduler.scheduler(), queue_policy.policy());
            tasks_cum.push(runner.tasks_completed);
        }
        per_tick_throughput(&tasks_cum)
    };

    // Faulted run
    let (faulted_tp, first_fault_tick) = {
        let solver =
            mafis::solver::lifelong_solver_from_name(solver_name, grid_area, num_agents).unwrap();
        let scenario = make_scenario(scenario_name);
        let fault_config = scenario.to_fault_config();
        let fault_schedule = scenario.generate_schedule(TICK_COUNT, num_agents);

        let mut runner = SimulationRunner::new(
            grid.clone(),
            zones.clone(),
            agents.clone(),
            solver,
            rng_after_placement.clone(),
            fault_config,
            fault_schedule,
        );

        let mut tasks_cum = Vec::with_capacity(TICK_COUNT as usize);
        let mut first_fault: Option<u64> = None;
        for t in 0..TICK_COUNT {
            let result = runner.tick(scheduler.scheduler(), queue_policy.policy());
            if first_fault.is_none() && !result.fault_events.is_empty() {
                first_fault = Some(t);
            }
            tasks_cum.push(runner.tasks_completed);
        }
        (per_tick_throughput(&tasks_cum), first_fault)
    };

    let baseline_smooth = rolling_average(&baseline_tp, SMOOTH_WINDOW);
    let faulted_smooth = rolling_average(&faulted_tp, SMOOTH_WINDOW);

    fs::create_dir_all("results/phase0_reliability").ok();
    let filename = format!(
        "results/phase0_reliability/trace_{solver_name}_{scenario_name}_n{num_agents}_seed{seed}.csv"
    );
    let mut f = File::create(&filename).unwrap();
    writeln!(
        f,
        "tick,baseline_tp,faulted_tp,baseline_smoothed,faulted_smoothed,ratio,smoothed_ratio"
    )
    .unwrap();

    let mut consec = 0_usize;
    let mut rapidity_tick: Option<usize> = None;
    let fault_start = first_fault_tick.unwrap_or(u64::MAX) as usize;

    for i in 0..baseline_tp.len() {
        let b = baseline_tp[i];
        let f_val = faulted_tp[i];
        let bs = baseline_smooth[i];
        let fs = faulted_smooth[i];
        let ratio = if b == 0.0 { 0.0 } else { f_val / b };
        let smoothed_ratio = if bs == 0.0 { 0.0 } else { fs / bs };

        if i >= fault_start {
            if smoothed_ratio >= RAPIDITY_THRESHOLD {
                consec += 1;
                if consec >= RAPIDITY_DWELL && rapidity_tick.is_none() {
                    rapidity_tick = Some(i + 1 - RAPIDITY_DWELL - fault_start);
                }
            } else {
                consec = 0;
            }
        }

        writeln!(f, "{i},{b:.4},{f_val:.4},{bs:.4},{fs:.4},{ratio:.4},{smoothed_ratio:.4}")
            .unwrap();
    }

    eprintln!(
        "\n=== Trace: {solver_name} / {scenario_name} / n={num_agents} / seed={seed} / {topo_name} ==="
    );
    eprintln!("  First fault tick: {first_fault_tick:?}");
    eprintln!("  Rapidity (our re-computation): {rapidity_tick:?}");
    eprintln!("  Wrote {filename}");

    if let Some(fs_tick) = first_fault_tick {
        let i = fs_tick as usize;
        if i < baseline_tp.len() {
            eprintln!("\n  === First 30 post-fault ticks ===");
            eprintln!("  tick  base_tp  flt_tp  base_sm  flt_sm  ratio  sm_ratio");
            for off in 0..30 {
                let t = i + off;
                if t >= baseline_tp.len() {
                    break;
                }
                let b = baseline_tp[t];
                let f_val = faulted_tp[t];
                let bs = baseline_smooth[t];
                let fs = faulted_smooth[t];
                let ratio = if b == 0.0 { 0.0 } else { f_val / b };
                let sm_ratio = if bs == 0.0 { 0.0 } else { fs / bs };
                eprintln!(
                    "  {t:4}  {b:7.3}  {f_val:6.3}  {bs:7.3}  {fs:6.3}  {ratio:5.2}  {sm_ratio:6.3}"
                );
            }
        }
    }
}

#[test]
#[ignore]
fn trace_rapidity_pibt_intermittent() {
    run_trace("pibt", "intermittent_80s80m15r", "warehouse_single_dock", 40, 42);
}

#[test]
#[ignore]
fn trace_rapidity_pibt_burst20() {
    run_trace("pibt", "burst_20pct", "warehouse_single_dock", 40, 42);
}

#[test]
#[ignore]
fn trace_rapidity_rhcr_intermittent() {
    run_trace("rhcr_pbs", "intermittent_80s80m15r", "warehouse_single_dock", 40, 42);
}

#[test]
#[ignore]
fn trace_rapidity_token_intermittent() {
    run_trace("token_passing", "intermittent_80s80m15r", "warehouse_single_dock", 40, 42);
}
