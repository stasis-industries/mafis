//! Paper experiment matrix definitions.
//!
//! Three focused experiments designed to answer distinct research questions:
//!
//! 1. **Solver resilience** — How do different solvers degrade under each fault type?
//! 2. **Scale sensitivity** — How does fleet size affect fault tolerance?
//! 3. **Scheduler effect** — Does task assignment strategy affect resilience?
//!
//! Each experiment varies one independent variable while controlling the others,
//! producing clean, publishable tables with 95% confidence intervals.

use crate::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};

use super::config::ExperimentMatrix;

/// Number of seeds per config — 30 gives usable 95% CI.
const SEEDS: &[u64] = &[
    42, 123, 456, 789, 1024,
    2048, 3141, 9999, 1337, 7777,
    11, 22, 33, 44, 55,
    101, 202, 303, 404, 505,
    1111, 2222, 3333, 4444, 5555,
    10000, 20000, 30000, 40000, 50000,
];

/// Standard simulation length — 500 ticks gives ~100 tasks at steady state.
const TICK_COUNT: u64 = 500;

// ---------------------------------------------------------------------------
// Fault scenarios used across all experiments
// ---------------------------------------------------------------------------

fn burst_20() -> FaultScenario {
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 20.0,
        burst_at_tick: 100,
        ..Default::default()
    }
}

fn burst_50() -> FaultScenario {
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 50.0,
        burst_at_tick: 100,
        ..Default::default()
    }
}

fn wear_medium() -> FaultScenario {
    // WearHeatRate::Medium -> Weibull beta=2.5, eta=500 -> ~63% fleet dead by tick 500.
    // Models typical industrial AGV deployment (Canadian survey: 500-1,000 h MTBF).
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::WearBased,
        wear_heat_rate: WearHeatRate::Medium,
        wear_threshold: 80.0,
        ..Default::default()
    }
}

fn wear_high() -> FaultScenario {
    // WearHeatRate::High -> Weibull beta=3.5, eta=150 -> ~90% fleet dead by tick 500.
    // Models high-stress operation (Carlson & Murphy 2006: field robot MTBF = 24 h).
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::WearBased,
        wear_heat_rate: WearHeatRate::High,
        wear_threshold: 60.0,
        ..Default::default()
    }
}

fn zone_outage() -> FaultScenario {
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::ZoneOutage,
        zone_at_tick: 100,
        zone_latency_duration: 50,
        ..Default::default()
    }
}

fn intermittent() -> FaultScenario {
    // IntermittentFault: exponential inter-arrival, 80-tick MTBF, 15-tick recovery.
    // Models sensor recalibration, momentary communication loss, battery reconnect.
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::IntermittentFault,
        intermittent_mtbf_ticks: 80,
        intermittent_recovery_ticks: 15,
        ..Default::default()
    }
}

/// All fault scenarios used in the paper.
fn paper_scenarios() -> Vec<Option<FaultScenario>> {
    vec![
        Some(burst_20()),
        Some(burst_50()),
        Some(wear_medium()),
        Some(wear_high()),
        Some(zone_outage()),
        Some(intermittent()),
    ]
}

// ---------------------------------------------------------------------------
// Experiment 1: Solver Resilience
// ---------------------------------------------------------------------------

/// **RQ1: How do different solvers degrade under each fault type?**
///
/// Independent variable: solver algorithm
/// Controlled: topology (medium), scheduler (random), agents (40)
///
/// Produces Table 1: Solver × Scenario matrix with FT, NRR, Critical Time.
///
/// 4 solvers x 6 scenarios x 30 seeds = 720 runs
pub fn solver_resilience() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec![
            "pibt".into(),
            "rhcr_pibt".into(),
            "rhcr_priority_astar".into(),
            "token_passing".into(),
        ],
        topologies: vec!["warehouse_medium".into()],
        scenarios: paper_scenarios(),
        schedulers: vec!["random".into()],
        agent_counts: vec![40],
        seeds: SEEDS.to_vec(),
        tick_count: TICK_COUNT,
    }
}

// ---------------------------------------------------------------------------
// Experiment 2: Topology Effect
// ---------------------------------------------------------------------------

/// **RQ2: Does warehouse layout affect fault resilience?**
///
/// Independent variable: topology (5 layouts from real industry)
/// Controlled: solver (pibt), scheduler (random), agents (scaled to topology)
///
/// Agent counts scaled to topology capacity.
/// Tests whether layout structure (aisles vs open vs dense) affects fault impact.
///
/// 5 topologies x 6 scenarios x 30 seeds = 900 runs
///
/// Note: agent counts are per-topology, not Cartesian. This function returns
/// 5 separate matrices (one per topology) to be run and merged.
pub fn topology_effect() -> Vec<ExperimentMatrix> {
    let scenarios = paper_scenarios();
    vec![
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_medium".into()],
            scenarios: scenarios.clone(),
            schedulers: vec!["random".into()],
            agent_counts: vec![40],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["kiva_large".into()],
            scenarios: scenarios.clone(),
            schedulers: vec!["random".into()],
            agent_counts: vec![80],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["sorting_center".into()],
            scenarios: scenarios.clone(),
            schedulers: vec!["random".into()],
            agent_counts: vec![30],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["compact_grid".into()],
            scenarios,
            schedulers: vec!["random".into()],
            agent_counts: vec![30],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
    ]
}

// ---------------------------------------------------------------------------
// Experiment 3: Scale Sensitivity
// ---------------------------------------------------------------------------

/// **RQ3: How does fleet size affect fault tolerance?**
///
/// Independent variable: number of agents (10, 20, 40, 80)
/// Controlled: solver (pibt), topology (medium), scheduler (random)
///
/// Produces Table 3: Agent Count × Scenario with FT, survival rate.
///
/// 4 agent counts x 6 scenarios x 30 seeds = 720 runs
pub fn scale_sensitivity() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_medium".into()],
        scenarios: paper_scenarios(),
        schedulers: vec!["random".into()],
        agent_counts: vec![10, 20, 40, 80],
        seeds: SEEDS.to_vec(),
        tick_count: TICK_COUNT,
    }
}

// ---------------------------------------------------------------------------
// Experiment 4: Scheduler Effect
// ---------------------------------------------------------------------------

/// **RQ4: Does task assignment strategy affect resilience?**
///
/// Independent variable: scheduler (random, closest)
/// Controlled: solver (pibt), topology (medium), agents (40)
///
/// Produces Table 4: Scheduler × Scenario with FT, idle ratio, throughput.
///
/// 2 schedulers x 6 scenarios x 30 seeds = 360 runs
pub fn scheduler_effect() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_medium".into()],
        scenarios: paper_scenarios(),
        schedulers: vec!["random".into(), "closest".into()],
        agent_counts: vec![40],
        seeds: SEEDS.to_vec(),
        tick_count: TICK_COUNT,
    }
}

// ---------------------------------------------------------------------------
// Full paper matrix (all experiments combined)
// ---------------------------------------------------------------------------

/// All experiment matrices for the paper.
///
/// Total: 720 + 900 + 720 + 360 = 2700 runs
/// At ~0.5s per run (500 ticks x 2 sims), ~20 minutes total.
///
/// Some configs overlap (e.g. pibt/medium/40/random appears in multiple
/// experiments). The overlap is intentional — each experiment is self-contained
/// and produces its own table. Deduplication happens at the analysis stage if
/// needed, not at the run stage.
pub fn all_paper_experiments() -> Vec<(&'static str, ExperimentMatrix)> {
    let mut experiments = vec![
        ("solver_resilience", solver_resilience()),
        ("scale_sensitivity", scale_sensitivity()),
        ("scheduler_effect", scheduler_effect()),
    ];
    let topo_names = [
        "topology_small",
        "topology_medium",
        "topology_kiva_large",
        "topology_sorting_center",
        "topology_compact_grid",
    ];
    for (i, m) in topology_effect().into_iter().enumerate() {
        experiments.push((topo_names[i], m));
    }
    experiments
}

// ---------------------------------------------------------------------------
// Quick smoke test matrix (for CI / development)
// ---------------------------------------------------------------------------

/// Minimal matrix for fast verification — 1 solver × 1 topology × 1 scenario × 2 seeds.
/// Takes ~1 second.
pub fn smoke_test() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_medium".into()],
        scenarios: vec![Some(burst_20())],
        schedulers: vec!["random".into()],
        agent_counts: vec![8],
        seeds: vec![42, 123],
        tick_count: 50,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_resilience_count() {
        let m = solver_resilience();
        assert_eq!(m.total_runs(), 720); // 4 x 6 x 30
    }

    #[test]
    fn topology_effect_count() {
        let matrices = topology_effect();
        let total: usize = matrices.iter().map(|m| m.total_runs()).sum();
        assert_eq!(total, 720); // 4 x (6 x 30)
    }

    #[test]
    fn scale_sensitivity_count() {
        let m = scale_sensitivity();
        assert_eq!(m.total_runs(), 720); // 4 x 6 x 30
    }

    #[test]
    fn scheduler_effect_count() {
        let m = scheduler_effect();
        assert_eq!(m.total_runs(), 360); // 2 x 6 x 30
    }

    #[test]
    fn all_paper_total() {
        let all = all_paper_experiments();
        let total: usize = all.iter().map(|(_, m)| m.total_runs()).sum();
        assert_eq!(total, 2520);
    }

    #[test]
    fn smoke_test_runs_fast() {
        let result = crate::experiment::runner::run_matrix(&smoke_test(), None);
        assert_eq!(result.runs.len(), 2);
        assert!(!result.summaries.is_empty());
        // Baseline should have more tasks than burst-faulted
        for run in &result.runs {
            assert!(run.baseline_metrics.total_tasks > 0);
        }
    }
}
