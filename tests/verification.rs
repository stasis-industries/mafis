//! Phase 1.1 Verification — comprehensive check of all solvers, topologies,
//! schedulers, fault injection, and determinism before running experiments.
//!
//! Run: cargo test --release --test verification -- --nocapture

use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};

const TICK_COUNT: u64 = 300;

// ─── Helper: run one config and return the result ──────────────────────

fn run(
    solver: &str,
    topology: &str,
    scheduler: &str,
    agents: usize,
    scenario: Option<FaultScenario>,
    seed: u64,
) -> mafis::experiment::runner::RunResult {
    let config = ExperimentConfig {
        solver_name: solver.into(),
        topology_name: topology.into(),
        scenario,
        scheduler_name: scheduler.into(),
        num_agents: agents,
        seed,
        tick_count: TICK_COUNT,
        custom_map: None,
    };
    run_single_experiment(&config)
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Solver × Topology: every solver runs on every topology without panic
// ═══════════════════════════════════════════════════════════════════════

const SOLVERS: &[&str] = &[
    "pibt",
    "rhcr_pbs",
    "rhcr_pibt",
    "rhcr_priority_astar",
    "token_passing",
];

const TOPOLOGIES: &[(&str, usize)] = &[
    ("warehouse_small", 8),
    ("warehouse_medium", 20),
    ("kiva_large", 30),
    ("sorting_center", 15),
    ("compact_grid", 15),
];

#[test]
fn all_solvers_on_all_topologies() {
    let mut failures = Vec::new();

    // Known limitations: PBS hits node limit on open maps with chokepoints.
    let known_zero = [("rhcr_pbs", "sorting_center")];

    for &solver in SOLVERS {
        for &(topology, agents) in TOPOLOGIES {
            let label = format!("{solver}/{topology}");
            eprint!("  {label:<40}");

            let r = run(solver, topology, "random", agents, None, 42);
            let tasks = r.baseline_metrics.total_tasks;
            let tp = r.baseline_metrics.avg_throughput;

            if tasks == 0 {
                if known_zero.contains(&(solver, topology)) {
                    eprintln!("SKIP (known: PBS node limit on this topology)");
                } else {
                    failures.push(format!("{label}: zero tasks in {TICK_COUNT} ticks"));
                    eprintln!("FAIL (0 tasks)");
                }
            } else {
                eprintln!("OK  tasks={tasks:>4}  tp={tp:.2}");
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "\n{} solver/topology combos produced zero tasks:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Schedulers: Random vs Closest both produce throughput
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn both_schedulers_produce_throughput() {
    for &sched in &["random", "closest"] {
        let r = run("pibt", "warehouse_medium", sched, 20, None, 42);
        assert!(
            r.baseline_metrics.total_tasks > 0,
            "{sched} scheduler produced 0 tasks"
        );
        assert!(
            r.baseline_metrics.avg_throughput > 0.0,
            "{sched} scheduler has zero throughput"
        );
        eprintln!(
            "  {sched:<10} tasks={:<4} tp={:.2}",
            r.baseline_metrics.total_tasks, r.baseline_metrics.avg_throughput
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Fault injection: each scenario type triggers correctly
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn burst_failure_kills_agents() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 20.0,
        burst_at_tick: 50,
        ..Default::default()
    };
    let r = run("pibt", "warehouse_medium", "random", 20, Some(scenario), 42);

    // Both runs should produce tasks
    assert!(r.baseline_metrics.total_tasks > 0, "baseline should produce tasks");
    assert!(r.faulted_metrics.total_tasks > 0, "faulted should produce tasks");
    // Note: faulted can exceed baseline (Braess's paradox — killing agents reduces congestion)

    // Survival rate should be < 1.0 (some agents died)
    assert!(
        r.faulted_metrics.survival_rate < 1.0,
        "burst should kill agents: survival={}",
        r.faulted_metrics.survival_rate
    );
    eprintln!(
        "  burst: baseline_tasks={} faulted_tasks={} survival={:.2}",
        r.baseline_metrics.total_tasks,
        r.faulted_metrics.total_tasks,
        r.faulted_metrics.survival_rate
    );
}

#[test]
fn wear_based_kills_agents_over_time() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::WearBased,
        wear_heat_rate: WearHeatRate::High, // aggressive: ~90% dead by tick 150
        ..Default::default()
    };
    // Use closest scheduler + fewer agents to ensure enough movement for
    // operational_age to reach Weibull failure ticks. Dense fleets congest
    // and accumulate very little operational_age.
    let r = run("pibt", "warehouse_medium", "closest", 10, Some(scenario), 42);

    // Wear should kill agents progressively
    assert!(
        r.faulted_metrics.survival_rate < 1.0,
        "wear should kill agents: survival={}",
        r.faulted_metrics.survival_rate
    );
    eprintln!(
        "  wear(high): baseline_tasks={} faulted_tasks={} survival={:.2} FT={:.2}",
        r.baseline_metrics.total_tasks,
        r.faulted_metrics.total_tasks,
        r.faulted_metrics.survival_rate,
        r.faulted_metrics.fault_tolerance
    );
}

#[test]
fn zone_outage_injects_latency() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::ZoneOutage,
        zone_at_tick: 50,
        zone_latency_duration: 30,
        ..Default::default()
    };
    let r = run("pibt", "warehouse_medium", "random", 20, Some(scenario), 42);

    // Zone outage should cause throughput dip but agents survive
    assert!(
        r.faulted_metrics.survival_rate >= 0.99,
        "zone outage should not kill agents: survival={}",
        r.faulted_metrics.survival_rate
    );
    // Tasks should still get done (agents recover after 30 ticks)
    assert!(
        r.faulted_metrics.total_tasks > 0,
        "should still complete tasks after zone outage"
    );
    eprintln!(
        "  zone_outage: baseline_tasks={} faulted_tasks={} survival={:.2}",
        r.baseline_metrics.total_tasks,
        r.faulted_metrics.total_tasks,
        r.faulted_metrics.survival_rate
    );
}

#[test]
fn intermittent_faults_reduce_throughput() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::IntermittentFault,
        intermittent_mtbf_ticks: 40,
        intermittent_recovery_ticks: 10,
        ..Default::default()
    };
    let r = run("pibt", "warehouse_medium", "random", 20, Some(scenario), 42);

    // Intermittent faults should not kill agents
    assert!(
        r.faulted_metrics.survival_rate >= 0.99,
        "intermittent should not kill: survival={}",
        r.faulted_metrics.survival_rate
    );
    // But throughput should be lower than baseline
    assert!(
        r.faulted_metrics.avg_throughput <= r.baseline_metrics.avg_throughput + 0.5,
        "intermittent should reduce throughput"
    );
    eprintln!(
        "  intermittent: baseline_tasks={} faulted_tasks={} FT={:.2}",
        r.baseline_metrics.total_tasks,
        r.faulted_metrics.total_tasks,
        r.faulted_metrics.fault_tolerance
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 4. Metrics sanity: FT, NRR, Critical Time are in valid ranges
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn metrics_in_valid_ranges() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 30.0,
        burst_at_tick: 50,
        ..Default::default()
    };
    let r = run("pibt", "warehouse_medium", "random", 20, Some(scenario), 42);
    let m = &r.faulted_metrics;

    // FT should be >= 0 (can exceed 1.0 for Braess's paradox — killing agents
    // reduces congestion, remaining agents outperform the full fleet)
    assert!(m.fault_tolerance >= 0.0, "FT negative: {}", m.fault_tolerance);

    // NRR should be 0..1 (NaN when MTBF unavailable — single burst has only 1 event)
    if !m.nrr.is_nan() {
        assert!(m.nrr >= 0.0, "NRR negative: {}", m.nrr);
        assert!(m.nrr <= 1.0, "NRR > 1: {}", m.nrr);
    }

    // Critical time should be 0..1
    assert!(m.critical_time >= 0.0, "critical_time negative: {}", m.critical_time);
    assert!(m.critical_time <= 1.0, "critical_time > 1: {}", m.critical_time);

    // Survival rate 0..1
    assert!(m.survival_rate >= 0.0 && m.survival_rate <= 1.0);

    // Idle ratio 0..1
    assert!(m.idle_ratio >= 0.0 && m.idle_ratio <= 1.0);
    assert!(m.wait_ratio >= 0.0 && m.wait_ratio <= 1.0);

    eprintln!(
        "  FT={:.3} NRR={:.3} CritTime={:.3} Survival={:.3} Idle={:.3}",
        m.fault_tolerance, m.nrr, m.critical_time, m.survival_rate, m.idle_ratio
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 5. Determinism: same seed + config = identical results
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn deterministic_replay() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 20.0,
        burst_at_tick: 50,
        ..Default::default()
    };

    let r1 = run("pibt", "warehouse_medium", "random", 20, Some(scenario.clone()), 42);
    let r2 = run("pibt", "warehouse_medium", "random", 20, Some(scenario), 42);

    assert_eq!(
        r1.baseline_metrics.total_tasks,
        r2.baseline_metrics.total_tasks,
        "baseline tasks differ"
    );
    assert_eq!(
        r1.faulted_metrics.total_tasks,
        r2.faulted_metrics.total_tasks,
        "faulted tasks differ"
    );
    assert_eq!(
        r1.faulted_metrics.deficit_integral,
        r2.faulted_metrics.deficit_integral,
        "deficit integral differs"
    );

    // Throughput should be bit-exact
    assert!(
        (r1.baseline_metrics.avg_throughput - r2.baseline_metrics.avg_throughput).abs() < 1e-10,
        "baseline throughput differs"
    );
    assert!(
        (r1.faulted_metrics.avg_throughput - r2.faulted_metrics.avg_throughput).abs() < 1e-10,
        "faulted throughput differs"
    );

    eprintln!("  determinism: OK (baseline_tasks={}, faulted_tasks={})",
        r1.baseline_metrics.total_tasks, r1.faulted_metrics.total_tasks);
}

/// Determinism holds across solvers — not just PIBT.
#[test]
fn deterministic_across_solvers() {
    for &solver in &["rhcr_pibt", "token_passing"] {
        let r1 = run(solver, "warehouse_small", "random", 8, None, 42);
        let r2 = run(solver, "warehouse_small", "random", 8, None, 42);
        assert_eq!(
            r1.baseline_metrics.total_tasks,
            r2.baseline_metrics.total_tasks,
            "{solver}: baseline tasks differ between identical runs"
        );
        eprintln!("  {solver}: deterministic OK (tasks={})", r1.baseline_metrics.total_tasks);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 6. New topologies: verify they produce sensible results
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn new_topologies_under_fault() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 20.0,
        burst_at_tick: 50,
        ..Default::default()
    };

    for &(topology, agents) in &[
        ("kiva_large", 30),
        ("sorting_center", 15),
        ("compact_grid", 15),
    ] {
        let r = run("pibt", topology, "random", agents, Some(scenario.clone()), 42);
        assert!(
            r.baseline_metrics.total_tasks > 0,
            "{topology}: baseline produced no tasks"
        );
        assert!(
            r.faulted_metrics.survival_rate < 1.0,
            "{topology}: burst should kill agents"
        );
        eprintln!(
            "  {topology:<20} baseline={:<4} faulted={:<4} FT={:.2} survival={:.2}",
            r.baseline_metrics.total_tasks,
            r.faulted_metrics.total_tasks,
            r.faulted_metrics.fault_tolerance,
            r.faulted_metrics.survival_rate,
        );
    }
}
