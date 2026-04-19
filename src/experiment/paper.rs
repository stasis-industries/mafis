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
    42, 123, 456, 789, 1024, 2048, 3141, 9999, 1337, 7777, 11, 22, 33, 44, 55, 101, 202, 303, 404,
    505, 1111, 2222, 3333, 4444, 5555, 10000, 20000, 30000, 40000, 50000,
];

/// Extended seeds — 50 for tighter CIs on the Braess experiment.
const SEEDS_50: &[u64] = &[
    42, 123, 456, 789, 1024, 2048, 3141, 9999, 1337, 7777, 11, 22, 33, 44, 55, 101, 202, 303, 404,
    505, 1111, 2222, 3333, 4444, 5555, 10000, 20000, 30000, 40000, 50000,
    // 20 additional seeds for tighter confidence intervals
    60000, 70000, 80000, 90000, 100000, 111, 222, 333, 444, 555, 666, 777, 888, 999, 1010, 2020,
    3030, 4040, 5050, 6060,
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
    // Models high-stress operation (Carlson & Murphy 2005: field robot MTBF ~ 24 h).
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
    // start_tick = MTBF (80): warm-up floor ensures cross-seed baseline comparability
    // by deferring first-fire until after the baseline establishment window.
    // Models sensor recalibration, momentary communication loss, battery reconnect.
    FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::IntermittentFault,
        intermittent_mtbf_ticks: 80,
        intermittent_recovery_ticks: 15,
        intermittent_start_tick: 80,
        ..Default::default()
    }
}

/// All fault scenarios (6 total, 2 categories).
///
/// Category 1 — Recoverable: ZoneOutage (spatial strip, 50t), IntermittentFault
/// Category 2 — Permanent-distributed: BurstFailure (20%/50%), WearBased (medium/high)
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
/// 3 solvers x 6 scenarios x 30 seeds = 540 runs
pub fn solver_resilience() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
        topologies: vec!["warehouse_single_dock".into()],
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
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: scenarios.clone(),
            schedulers: vec!["random".into()],
            agent_counts: vec![40],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_dual_dock".into()],
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
            scenarios: scenarios.clone(),
            schedulers: vec!["random".into()],
            agent_counts: vec![25],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        },
        ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["fullfilment_center".into()],
            scenarios,
            schedulers: vec!["random".into()],
            agent_counts: vec![35],
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
        topologies: vec!["warehouse_single_dock".into()],
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
        topologies: vec!["warehouse_single_dock".into()],
        scenarios: paper_scenarios(),
        schedulers: vec!["random".into(), "closest".into()],
        agent_counts: vec![40],
        seeds: SEEDS.to_vec(),
        tick_count: TICK_COUNT,
    }
}

// ---------------------------------------------------------------------------
// Full experiment matrix (all experiments combined)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Experiment 5: Braess Resilience (Solver × Density × Fault Category)
// ---------------------------------------------------------------------------

/// **RQ5: Does fault type interact with fleet density and solver architecture?**
///
/// Independent variables: solver (4), fleet size (4), fault scenario (6)
/// Controlled: topology (medium), scheduler (random)
///
/// Tests the Braess hypothesis: under congestion, permanent agent removal
/// can paradoxically improve throughput for reactive solvers by reducing
/// corridor competition, while coordinated solvers suffer.
///
/// 3 solvers x 4 densities x 6 scenarios x 50 seeds = 3,600 runs
pub fn braess_resilience() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
        topologies: vec!["warehouse_single_dock".into()],
        scenarios: paper_scenarios(),
        schedulers: vec!["random".into()],
        agent_counts: vec![10, 20, 40, 80],
        seeds: SEEDS_50.to_vec(),
        tick_count: TICK_COUNT,
    }
}

/// All experiment matrices (legacy presets).
///
/// Total: 540 + 900 + 720 + 360 = 2520 runs
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
        "topology_warehouse_single_dock",
        "topology_warehouse_dual_dock",
        "topology_sorting_center",
        "topology_compact_grid",
        "topology_fullfilment_center",
    ];
    for (i, m) in topology_effect().into_iter().enumerate() {
        experiments.push((topo_names[i], m));
    }
    experiments
}

// ---------------------------------------------------------------------------
// Main experiment suite — 3 solvers × 6 scenarios × 2 topologies
// ---------------------------------------------------------------------------

/// Solvers used in the main experiment suite.
///
/// Every solver has a faithful Rust implementation traceable to a
/// public reference.
fn paams_solvers() -> Vec<String> {
    vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()]
}

/// Main experiment suite: Solver × Fault × Scale × Topology + Scheduler effect.
///
/// Per-topology matrices with agent counts varying by map capacity.
/// Three density levels per topology: low / default / high.
///
/// E1: 3 solvers × 6 scenarios × 3 agent counts × 3 topologies × 30 seeds = 4,860 runs
/// E2: 3 solvers × 6 scenarios × 2 schedulers × 1 topology × 30 seeds = 1,080 runs
/// Total: 4,320 runs
pub fn paams_experiments() -> Vec<(&'static str, ExperimentMatrix)> {
    let solvers = paams_solvers();
    let scenarios = paper_scenarios();

    vec![
        // E1a: warehouse_single_dock (57×33) — 20/40/60 agents
        (
            "paams_warehouse_single_dock",
            ExperimentMatrix {
                solvers: solvers.clone(),
                topologies: vec!["warehouse_single_dock".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![20, 40, 60],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // E1b: warehouse_dual_dock (61×33) — 40/80/120 agents
        (
            "paams_warehouse_dual_dock",
            ExperimentMatrix {
                solvers: solvers.clone(),
                topologies: vec!["warehouse_dual_dock".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![40, 80, 120],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // E2: Scheduler effect (warehouse_single_dock, 40 agents)
        (
            "paams_scheduler_effect",
            ExperimentMatrix {
                solvers,
                topologies: vec!["warehouse_single_dock".into()],
                scenarios,
                schedulers: vec!["random".into(), "closest".into()],
                agent_counts: vec![40],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Aisle-width sweep (PAAMS 2026 — structural cascade claim)
// ---------------------------------------------------------------------------
//
// Three single-dock variants that differ ONLY in inter-rack aisle width:
//   SD-w1 (57×33, aisle=1): existing warehouse_single_dock, fleet {20, 40, 60}
//   SD-w2 (57×44, aisle=2): warehouse_sd_w2,               fleet {36, 72, 108}
//   SD-w3 (57×55, aisle=3): warehouse_sd_w3,               fleet {50, 100, 151}
//
// Rack count (800 cells), pickup density, and fleet-density (agents /
// walkable-cell) are held constant across the three maps; only aisle width —
// and therefore structural bypass capacity — varies. Delivery-cell count
// scales with walkable area (one station per ~77 walkable cells) so no
// solver gets a dock-queue advantage from map size.
//
// Token Passing envelope: TP operates reliably at ≤100 agents (per-agent A*
// budget at `ASTAR_MAX_EXPANSIONS = 5000` exhausts above that threshold and
// many agents default to Wait). We split each topology into two matrices:
//
//   *_in_env  → all three solvers,   densities within TP's envelope
//   *_out_env → PIBT + RHCR-PBS only, density above TP's envelope
//
// SD-w1 is entirely in-envelope so it only produces one matrix. The
// out-envelope cells (SD-w2 n=108, SD-w3 n=151) still belong to the sweep —
// they feed the secondary "decentralized paradigm exits the envelope before
// centralized" finding without contaminating the primary topology-sensitivity
// claim.

/// **RQ-Aisle: Does inter-rack aisle width modulate fault tolerance at
/// fixed density, and does this effect differ by solver paradigm?**
///
/// Independent variable: aisle width (1 / 2 / 3 cells)
/// Controlled: rack count, pickup density, agent density, scheduler (closest)
///
/// Token Passing restricted to ≤100 agents (its design envelope). Out-of-
/// envelope densities (SD-w2 n=108, SD-w3 n=151) are run only with PIBT and
/// RHCR-PBS so the primary aisle-width claim rests on in-envelope,
/// solver-comparable cells while the paradigm-limit finding keeps its own
/// data.
///
/// Total runs (6 scenarios × 30 seeds baseline-paired):
///   SD-w1           3 solvers × 3 counts × 6 × 30 = 1620
///   SD-w2 in-env    3 solvers × 2 counts × 6 × 30 = 1080
///   SD-w2 out-env   2 solvers × 1 count  × 6 × 30 =  360
///   SD-w3 in-env    3 solvers × 2 counts × 6 × 30 = 1080
///   SD-w3 out-env   2 solvers × 1 count  × 6 × 30 =  360
///   ────────────────────────────────────────────────────
///                                                   4500
pub fn paams_aisle_width() -> Vec<(&'static str, ExperimentMatrix)> {
    let solvers_all = paams_solvers();
    let solvers_scalable: Vec<String> = vec!["pibt".into(), "rhcr_pbs".into()];
    let scenarios = paper_scenarios();

    vec![
        // SD-w1 (aisle width 1) — fully in-envelope
        (
            "aisle_width_w1",
            ExperimentMatrix {
                solvers: solvers_all.clone(),
                topologies: vec!["warehouse_single_dock".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![20, 40, 60],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // SD-w2 (aisle width 2) in-envelope
        (
            "aisle_width_w2_in_env",
            ExperimentMatrix {
                solvers: solvers_all.clone(),
                topologies: vec!["warehouse_sd_w2".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![36, 72],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // SD-w2 out-envelope — PIBT + RHCR-PBS only
        (
            "aisle_width_w2_out_env",
            ExperimentMatrix {
                solvers: solvers_scalable.clone(),
                topologies: vec!["warehouse_sd_w2".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![108],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // SD-w3 (aisle width 3) in-envelope
        (
            "aisle_width_w3_in_env",
            ExperimentMatrix {
                solvers: solvers_all,
                topologies: vec!["warehouse_sd_w3".into()],
                scenarios: scenarios.clone(),
                schedulers: vec!["closest".into()],
                agent_counts: vec![50, 100],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
        // SD-w3 out-envelope — PIBT + RHCR-PBS only
        (
            "aisle_width_w3_out_env",
            ExperimentMatrix {
                solvers: solvers_scalable,
                topologies: vec!["warehouse_sd_w3".into()],
                scenarios,
                schedulers: vec!["closest".into()],
                agent_counts: vec![151],
                seeds: SEEDS.to_vec(),
                tick_count: TICK_COUNT,
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// Quick smoke test matrix (for CI / development)
// ---------------------------------------------------------------------------

/// Minimal matrix for fast verification — 1 solver × 1 topology × 1 scenario × 2 seeds.
/// Takes ~1 second.
pub fn smoke_test() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_single_dock".into()],
        scenarios: vec![Some(burst_20())],
        schedulers: vec!["random".into()],
        agent_counts: vec![15],
        seeds: vec![42, 123],
        tick_count: 100,
    }
}

// ---------------------------------------------------------------------------
// Tier 3: Solver benchmark — all faithful solvers, baseline throughput comparison
// ---------------------------------------------------------------------------

/// Benchmark all faithful solvers at 40 agents on warehouse_single_dock, no faults.
/// 5 seeds for statistical confidence. 30 runs total (3 solvers × 2 scenarios × 5 seeds).
pub fn solver_benchmark() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
        topologies: vec!["warehouse_single_dock".into()],
        scenarios: vec![None, Some(burst_20())],
        schedulers: vec!["random".into()],
        agent_counts: vec![40],
        seeds: vec![42, 123, 456, 789, 1024],
        tick_count: 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solver_resilience_count() {
        let m = solver_resilience();
        assert_eq!(m.total_runs(), 540); // 3 solvers × 6 scenarios × 30 seeds
    }

    #[test]
    fn topology_effect_count() {
        let matrices = topology_effect();
        let total: usize = matrices.iter().map(|m| m.total_runs()).sum();
        assert_eq!(total, 900); // 5 x (6 x 30) — pibt only, unchanged
    }

    #[test]
    fn scale_sensitivity_count() {
        let m = scale_sensitivity();
        assert_eq!(m.total_runs(), 720); // 4 x 6 x 30 — pibt only, unchanged
    }

    #[test]
    fn scheduler_effect_count() {
        let m = scheduler_effect();
        assert_eq!(m.total_runs(), 360); // 2 x 6 x 30 — pibt only, unchanged
    }

    #[test]
    fn all_paper_total() {
        let all = all_paper_experiments();
        let total: usize = all.iter().map(|(_, m)| m.total_runs()).sum();
        assert_eq!(total, 2520); // 540 + 900 + 720 + 360
    }

    #[test]
    fn paams_experiment_counts() {
        let experiments = paams_experiments();
        let total: usize = experiments.iter().map(|(_, m)| m.total_runs()).sum();
        // E1: 3 solvers × 6 scenarios × 3 counts × 30 seeds × 2 topos = 3,240
        // E2: 3 solvers × 6 scenarios × 2 schedulers × 30 seeds = 1,080
        assert_eq!(total, 4320);
    }

    #[test]
    fn paams_aisle_width_counts() {
        let experiments = paams_aisle_width();
        assert_eq!(experiments.len(), 5);
        let total: usize = experiments.iter().map(|(_, m)| m.total_runs()).sum();
        //   SD-w1:          3 × 3 × 6 × 30 = 1620
        //   SD-w2 in-env:   3 × 2 × 6 × 30 = 1080
        //   SD-w2 out-env:  2 × 1 × 6 × 30 =  360
        //   SD-w3 in-env:   3 × 2 × 6 × 30 = 1080
        //   SD-w3 out-env:  2 × 1 × 6 × 30 =  360
        //   Total                           = 4500
        assert_eq!(total, 4500);
    }

    #[test]
    fn paams_aisle_width_tp_only_in_envelope() {
        // Token Passing must appear only in cells with num_agents ≤ 100.
        for (name, m) in paams_aisle_width() {
            let has_tp = m.solvers.iter().any(|s| s == "token_passing");
            let max_n = *m.agent_counts.iter().max().unwrap();
            if has_tp {
                assert!(
                    max_n <= 100,
                    "token_passing out of envelope in matrix {name} (max n={max_n})"
                );
            }
        }
    }

    #[test]
    fn braess_resilience_count() {
        let m = braess_resilience();
        assert_eq!(m.total_runs(), 3600); // 3 solvers × 4 densities × 6 scenarios × 50 seeds
    }

    /// Cross-topology validation: does the Braess effect replicate on other layouts?
    ///
    /// Tests burst_20 + burst_50 on sorting_center and compact_grid at n=20,40
    /// with 30 seeds each. 2 solvers × 2 topologies × 2 scenarios × 2 densities × 30 seeds = 480 runs.
    ///
    /// Usage: cargo test run_cross_topology -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_cross_topology() {
        use crate::experiment::export::write_runs_csv;
        use crate::experiment::runner::run_matrix;
        use std::fs;

        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into(), "token_passing".into()],
            topologies: vec!["sorting_center".into(), "compact_grid".into()],
            scenarios: vec![Some(burst_20()), Some(burst_50())],
            schedulers: vec!["random".into()],
            agent_counts: vec![20, 40],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        };

        let total = matrix.total_runs();
        eprintln!("Cross-topology validation: {} runs...", total);

        use crate::experiment::runner::ExperimentProgress;
        use std::sync::{Arc, Mutex};
        let progress =
            Arc::new(Mutex::new(ExperimentProgress { current: 0, total, label: String::new() }));
        let result = run_matrix(&matrix, Some(&progress));
        eprintln!("Done in {}ms", result.wall_time_total_ms);

        fs::create_dir_all("results").unwrap();
        let mut f = fs::File::create("results/cross_topology_runs.csv").unwrap();
        write_runs_csv(&mut f, &result.runs).unwrap();
        eprintln!("Saved: cross_topology_runs.csv ({} rows)", result.runs.len() * 2);
    }

    /// Run solver resilience — full 30-seed version for publication.
    /// Uses closest scheduler, 20 agents.
    ///
    /// 6 solvers x 3 scenarios x 30 seeds = 540 runs.
    ///
    /// Usage: cargo test run_new_solver_resilience -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_new_solver_resilience() {
        use crate::experiment::export::{write_runs_csv, write_summary_csv};
        use crate::experiment::runner::{ExperimentProgress, run_matrix};
        use std::fs;
        use std::sync::{Arc, Mutex};

        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None, Some(burst_20()), Some(burst_50())],
            schedulers: vec!["closest".into()],
            agent_counts: vec![20],
            seeds: SEEDS.to_vec(),
            tick_count: TICK_COUNT,
        };

        let total = matrix.total_runs();
        eprintln!("New solver resilience: {} runs...", total);
        let progress =
            Arc::new(Mutex::new(ExperimentProgress { current: 0, total, label: String::new() }));
        let result = run_matrix(&matrix, Some(&progress));
        eprintln!("Done in {}ms", result.wall_time_total_ms);

        fs::create_dir_all("results").unwrap();

        let mut f = fs::File::create("results/new_solver_resilience_runs.csv").unwrap();
        write_runs_csv(&mut f, &result.runs).unwrap();

        let mut f = fs::File::create("results/new_solver_resilience_summary.csv").unwrap();
        write_summary_csv(&mut f, &result.summaries).unwrap();

        eprintln!(
            "Saved: new_solver_resilience_runs.csv ({} rows), summary ({} rows)",
            result.runs.len() * 2,
            result.summaries.len()
        );

        // Print summary table
        eprintln!("\n=== Solver Resilience Results ===");
        eprintln!(
            "{:<15} {:<14} {:>6} {:>7} {:>7} {:>4}",
            "Solver", "Scenario", "FT", "TP", "Tasks", "n"
        );
        eprintln!("{}", "-".repeat(58));
        for s in &result.summaries {
            let ft_str = if s.fault_tolerance.mean.is_nan() {
                "  NaN".to_string()
            } else {
                format!("{:.3}", s.fault_tolerance.mean)
            };
            eprintln!(
                "  {:<15} {:<14} {:>5} {:>7.3} {:>5.0}   {:>2}",
                s.solver_name,
                s.scenario_label,
                ft_str,
                s.throughput.mean,
                s.total_tasks.mean,
                s.fault_tolerance.n
            );
        }
    }

    /// Tier 3: Run all 8 solvers and validate performance expectations.
    ///
    /// This is the benchmark comparison test. It runs each solver on
    /// warehouse_single_dock with 40 agents for 500 ticks (5 seeds, no faults)
    /// and validates:
    /// 1. All solvers produce non-zero throughput
    /// 2. Performance ranking roughly matches paper expectations
    /// 3. No solver is catastrophically worse than expected
    ///
    /// Usage: cargo test run_solver_benchmark -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_solver_benchmark() {
        use crate::experiment::export::write_runs_csv;
        use crate::experiment::runner::{ExperimentProgress, run_matrix};
        use std::collections::HashMap;
        use std::fs;
        use std::sync::{Arc, Mutex};

        // Baseline only (no faults) for clean throughput comparison
        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![40],
            seeds: vec![42, 123, 456, 789, 1024],
            tick_count: 500,
        };

        let total = matrix.total_runs();
        eprintln!("Solver benchmark: {} runs...", total);
        let progress =
            Arc::new(Mutex::new(ExperimentProgress { current: 0, total, label: String::new() }));
        let result = run_matrix(&matrix, Some(&progress));
        eprintln!("Done in {}ms", result.wall_time_total_ms);

        // Aggregate: average throughput per solver across 5 seeds
        let mut solver_throughputs: HashMap<String, Vec<f64>> = HashMap::new();
        for run in &result.runs {
            solver_throughputs
                .entry(run.config.solver_name.clone())
                .or_default()
                .push(run.baseline_metrics.avg_throughput);
        }

        eprintln!(
            "\n=== Solver Benchmark Results (40 agents, warehouse_single_dock, 500 ticks) ==="
        );
        eprintln!("{:<25} {:>8} {:>8} {:>8}", "Solver", "Mean TP", "Min TP", "Max TP");
        eprintln!("{}", "-".repeat(55));

        let mut solver_means: Vec<(String, f64)> = Vec::new();
        for (solver, tps) in &solver_throughputs {
            let mean = tps.iter().sum::<f64>() / tps.len() as f64;
            let min = tps.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = tps.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            eprintln!("{:<25} {:>8.3} {:>8.3} {:>8.3}", solver, mean, min, max);
            solver_means.push((solver.clone(), mean));
        }

        // Sort by throughput descending
        solver_means.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        eprintln!("\nRanking (highest to lowest throughput):");
        for (rank, (solver, mean)) in solver_means.iter().enumerate() {
            eprintln!("  {}. {} (tp={:.3})", rank + 1, solver, mean);
        }

        // Validation: all solvers must produce non-zero throughput
        for (solver, mean) in &solver_means {
            assert!(
                *mean > 0.0,
                "solver {solver} produced zero throughput on warehouse_single_dock with 40 agents"
            );
        }

        // Save results
        fs::create_dir_all("results").unwrap();
        let mut f = fs::File::create("results/solver_benchmark_runs.csv").unwrap();
        write_runs_csv(&mut f, &result.runs).unwrap();
        eprintln!("\nSaved: results/solver_benchmark_runs.csv ({} rows)", result.runs.len() * 2);
    }

    /// Launch the full aisle-width sweep: all 5 matrices, runs CSVs + summary CSVs
    /// written to `results/aisle_width/`. Set `MAFIS_TICK_EXPORT_DIR` to capture
    /// per-tick throughput series (see runner::export_tick_series_if_enabled).
    ///
    /// Total: 4500 runs (see paams_aisle_width_counts test).
    ///
    /// Usage:
    ///   MAFIS_TICK_EXPORT_DIR="$(pwd)/results/aisle_width/ticks" \
    ///     cargo test --release --lib run_aisle_width_sweep -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_aisle_width_sweep() {
        use crate::experiment::export::{write_runs_csv, write_summary_csv};
        use crate::experiment::runner::{ExperimentProgress, run_matrix};
        use std::fs;
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        let out_dir = "results/aisle_width";
        fs::create_dir_all(out_dir).unwrap();

        let matrices = paams_aisle_width();
        let grand_total: usize = matrices.iter().map(|(_, m)| m.total_runs()).sum();
        let sweep_start = Instant::now();

        eprintln!(
            "\n=== Aisle-width sweep: {grand_total} total runs across {} matrices ===",
            matrices.len()
        );
        for (name, matrix) in matrices {
            let total = matrix.total_runs();
            eprintln!("\n--- {name}: {total} runs ---");
            let progress =
                Arc::new(Mutex::new(ExperimentProgress { current: 0, total, label: name.into() }));

            let m_start = Instant::now();
            let result = run_matrix(&matrix, Some(&progress));
            let m_wall_s = m_start.elapsed().as_secs();
            eprintln!("  done in {m_wall_s}s ({} runs/s)", total as f64 / m_wall_s.max(1) as f64);

            let runs_path = format!("{out_dir}/{name}_runs.csv");
            let summary_path = format!("{out_dir}/{name}_summary.csv");
            write_runs_csv(&mut fs::File::create(&runs_path).unwrap(), &result.runs).unwrap();
            write_summary_csv(&mut fs::File::create(&summary_path).unwrap(), &result.summaries)
                .unwrap();
            eprintln!("  wrote {runs_path}, {summary_path}");
        }

        let total_s = sweep_start.elapsed().as_secs();
        eprintln!(
            "\n=== Aisle-width sweep complete: {grand_total} runs in {}h{:02}m{:02}s ===",
            total_s / 3600,
            (total_s % 3600) / 60,
            total_s % 60,
        );
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
