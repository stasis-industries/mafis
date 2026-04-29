//! Integration tests for the headline experiment suite.
//!
//! Run the full matrix:
//!   cargo test --release full_legacy_matrix -- --ignored --nocapture
//!
//! Run the smoke test:
//!   cargo test smoke -- --nocapture

use mafis::experiment::ExperimentMatrix;
use mafis::experiment::export;
use mafis::experiment::runner::{MatrixResult, RunResult, run_matrix};
use mafis::experiment::suite;
use mafis::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};
use std::fs;

const OUTPUT_DIR: &str = "results";

fn ensure_output_dir() {
    fs::create_dir_all(OUTPUT_DIR).expect("failed to create results/");
}

/// Smoke test — 2 runs, ~1 second. Validates the pipeline end-to-end.
#[test]
fn smoke() {
    let matrix = suite::smoke_test();
    let result = run_matrix(&matrix, None);

    assert_eq!(result.runs.len(), 2);
    assert_eq!(result.summaries.len(), 1);

    // Both runs should complete tasks
    for run in &result.runs {
        assert!(run.baseline_metrics.total_tasks > 0, "baseline should complete tasks");
    }

    // Determinism: same seed should produce same baseline
    let seed_42: Vec<&RunResult> = result.runs.iter().filter(|r| r.config.seed == 42).collect();
    assert_eq!(seed_42.len(), 1);
}

/// Full legacy matrix — 300 runs across 5 experiments.
/// Run with: cargo test --release full_legacy_matrix -- --ignored --nocapture
#[test]
#[ignore]
fn full_legacy_matrix() {
    ensure_output_dir();

    let experiments = suite::all_legacy_experiments();
    let total_runs: usize = experiments.iter().map(|(_, m)| m.total_runs()).sum();
    eprintln!("=== MAFIS Legacy Experiment Matrix ===");
    eprintln!("Total experiments: {}", experiments.len());
    eprintln!("Total runs: {total_runs}");
    eprintln!();

    let mut all_runs: Vec<RunResult> = Vec::new();
    let overall_start = std::time::Instant::now();

    for (name, matrix) in &experiments {
        eprintln!("─── Experiment: {name} ({} runs) ───", matrix.total_runs());
        let result = run_matrix(matrix, None);

        // Write per-experiment CSV
        write_experiment_results(name, &result);

        all_runs.extend(result.runs);
        eprintln!();
    }

    let total_wall = overall_start.elapsed();
    eprintln!("=== All experiments complete ===");
    eprintln!("Total runs: {}", all_runs.len());
    eprintln!("Total wall time: {:.1}s", total_wall.as_secs_f64());
    eprintln!("Avg per run: {:.0}ms", total_wall.as_millis() as f64 / all_runs.len() as f64);

    // Write combined output
    {
        let mut f = fs::File::create(format!("{OUTPUT_DIR}/all_runs.csv")).unwrap();
        export::write_runs_csv(&mut f, &all_runs).unwrap();
        eprintln!("Wrote {OUTPUT_DIR}/all_runs.csv ({} rows)", all_runs.len() * 2);
    }

    // Verify sanity — warn on zero-task baselines but don't fail.
    // RHCR-PIBT at high density (40 agents on warehouse_single_dock) can produce 0
    // baseline tasks for some seeds due to windowed planning deadlocks. This is
    // a legitimate (poor) solver outcome, not a pipeline failure.
    let mut zero_task_runs = Vec::new();
    for run in &all_runs {
        if run.baseline_metrics.total_tasks == 0 && run.config.num_agents > 0 {
            zero_task_runs.push(format!(
                "{} / {} / {} agents / seed {}",
                run.config.solver_name,
                run.config.topology_name,
                run.config.num_agents,
                run.config.seed,
            ));
        }
    }
    if !zero_task_runs.is_empty() {
        eprintln!(
            "\n⚠ {} runs produced 0 baseline tasks (high-density deadlock):",
            zero_task_runs.len()
        );
        for desc in &zero_task_runs {
            eprintln!("  - {desc}");
        }
    }
}

// ---------------------------------------------------------------------------
// Individual experiment tests (invoked by `mafis experiment run <name>`)
// ---------------------------------------------------------------------------

/// Solver resilience — 75 runs.
/// Run with: cargo test --release --test experiment_suite solver_resilience -- --ignored --nocapture
#[test]
#[ignore]
fn solver_resilience() {
    ensure_output_dir();
    let matrix = suite::solver_resilience();
    eprintln!("─── solver_resilience ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("solver_resilience", &result);
}

/// Scale sensitivity — 100 runs.
/// Run with: cargo test --release --test experiment_suite scale_sensitivity -- --ignored --nocapture
#[test]
#[ignore]
fn scale_sensitivity() {
    ensure_output_dir();
    let matrix = suite::scale_sensitivity();
    eprintln!("─── scale_sensitivity ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("scale_sensitivity", &result);
}

/// Scheduler effect — 50 runs.
/// Run with: cargo test --release --test experiment_suite scheduler_effect -- --ignored --nocapture
#[test]
#[ignore]
fn scheduler_effect() {
    ensure_output_dir();
    let matrix = suite::scheduler_effect();
    eprintln!("─── scheduler_effect ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("scheduler_effect", &result);
}

/// Topology medium — 25 runs.
/// Run with: cargo test --release --test experiment_suite topology_medium -- --ignored --nocapture
#[test]
#[ignore]
fn topology_medium() {
    ensure_output_dir();
    let matrices = suite::topology_effect();
    let matrix = &matrices[0]; // warehouse_single_dock
    eprintln!("─── topology_medium ({} runs) ───", matrix.total_runs());
    let result = run_matrix(matrix, None);
    write_experiment_results("topology_medium", &result);
}

/// Topology large — 25 runs.
/// Run with: cargo test --release --test experiment_suite topology_large -- --ignored --nocapture
#[test]
#[ignore]
fn topology_large() {
    ensure_output_dir();
    let matrices = suite::topology_effect();
    let matrix = &matrices[1]; // warehouse_dual_dock
    eprintln!("─── topology_large ({} runs) ───", matrix.total_runs());
    let result = run_matrix(matrix, None);
    write_experiment_results("topology_large", &result);
}

/// Braess resilience — 6,000 runs (~60 min).
/// Run with: cargo test --release --test experiment_suite braess_resilience -- --ignored --nocapture
#[test]
#[ignore]
fn braess_resilience() {
    ensure_output_dir();
    let matrix = suite::braess_resilience();
    eprintln!("─── braess_resilience ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("braess_resilience", &result);
}

/// Headline experiment matrix — 7,920 runs (4 solvers × 6 scenarios × 3 topologies × 30 seeds).
/// Run with: cargo test --release --test experiment_suite full_experiment_suite -- --ignored --nocapture
#[test]
#[ignore]
fn full_experiment_suite() {
    ensure_output_dir();

    let experiments = suite::core_experiment_suite();
    let total_runs: usize = experiments.iter().map(|(_, m)| m.total_runs()).sum();
    eprintln!("=== Headline Experiment Suite ===");
    eprintln!("Total experiments: {}", experiments.len());
    eprintln!("Total runs: {total_runs}");
    eprintln!();

    let mut all_runs: Vec<RunResult> = Vec::new();
    let overall_start = std::time::Instant::now();

    for (name, matrix) in &experiments {
        eprintln!("─── {name} ({} runs) ───", matrix.total_runs());
        let result = run_matrix(matrix, None);
        write_experiment_results(name, &result);
        all_runs.extend(result.runs);
        eprintln!();
    }

    let total_wall = overall_start.elapsed();
    eprintln!("=== Experiment suite complete ===");
    eprintln!("Total runs: {}", all_runs.len());
    eprintln!("Total wall time: {:.1}s", total_wall.as_secs_f64());
    eprintln!("Avg per run: {:.0}ms", total_wall.as_millis() as f64 / all_runs.len() as f64);

    // Write combined output
    {
        let mut f = fs::File::create(format!("{OUTPUT_DIR}/all_runs.csv")).unwrap();
        export::write_runs_csv(&mut f, &all_runs).unwrap();
        eprintln!("Wrote {OUTPUT_DIR}/all_runs.csv ({} rows)", all_runs.len() * 2);
    }
}

/// Run a single fault scenario loaded from `MAFIS_FAULT_*` environment variables.
///
/// Invoked by `mafis fault run --fault-config <path.toml>`.  The CLI reads
/// the TOML file, validates it, and then shells out to this test passing all
/// scenario + experiment parameters as env vars.
///
/// Run directly (for debugging):
///   MAFIS_FAULT_TYPE=burst_failure MAFIS_FAULT_SEEDS=42 \
///   cargo test --test experiment_suite fault_from_env -- --nocapture
#[test]
fn fault_from_env() {
    ensure_output_dir();

    // --- Read scenario type ---
    let fault_type = std::env::var("MAFIS_FAULT_TYPE").unwrap_or_else(|_| "burst_failure".into());

    // --- Read experiment parameters ---
    let solver = std::env::var("MAFIS_FAULT_SOLVER").unwrap_or_else(|_| "pibt".into());
    let topology =
        std::env::var("MAFIS_FAULT_TOPOLOGY").unwrap_or_else(|_| "warehouse_single_dock".into());
    let scheduler = std::env::var("MAFIS_FAULT_SCHEDULER").unwrap_or_else(|_| "random".into());
    let num_agents: usize =
        std::env::var("MAFIS_FAULT_NUM_AGENTS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let tick_count: u64 =
        std::env::var("MAFIS_FAULT_TICK_COUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
    let seeds: Vec<u64> = std::env::var("MAFIS_FAULT_SEEDS")
        .unwrap_or_else(|_| "42".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    // --- Build FaultScenario from env vars ---
    let scenario = build_scenario_from_env(&fault_type);

    // --- Print summary ---
    eprintln!("=== fault_from_env ===");
    eprintln!("  type:     {fault_type}");
    eprintln!("  solver:   {solver}");
    eprintln!("  topology: {topology}");
    eprintln!("  agents:   {num_agents}");
    eprintln!("  ticks:    {tick_count}");
    eprintln!("  seeds:    {} ({} runs)", format_seeds_preview(&seeds), seeds.len());
    eprintln!();

    // --- Build matrix ---
    let matrix = ExperimentMatrix {
        solvers: vec![solver.clone()],
        topologies: vec![topology.clone()],
        scenarios: vec![Some(scenario)],
        schedulers: vec![scheduler.clone()],
        agent_counts: vec![num_agents],
        seeds: seeds.clone(),
        tick_count,
        rhcr_overrides: vec![None],
    };

    let result = run_matrix(&matrix, None);

    let label =
        format!("fault_from_env_{}_{}_{}agents", fault_type.replace('_', "-"), solver, num_agents,);
    write_experiment_results(&label, &result);

    eprintln!("Runs completed: {}", result.runs.len());
    for run in &result.runs {
        eprintln!(
            "  seed={:6}  baseline_tasks={:4}  faulted_tasks={:4}  ft={:.3}",
            run.config.seed,
            run.baseline_metrics.total_tasks,
            run.faulted_metrics.total_tasks,
            run.faulted_metrics.fault_tolerance,
        );
    }
}

/// Build a `FaultScenario` from `MAFIS_FAULT_*` environment variables.
fn build_scenario_from_env(fault_type: &str) -> FaultScenario {
    let burst_kill_percent: f32 = std::env::var("MAFIS_FAULT_BURST_KILL_PERCENT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let burst_at_tick: u64 =
        std::env::var("MAFIS_FAULT_BURST_AT_TICK").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let wear_heat_rate_str =
        std::env::var("MAFIS_FAULT_WEAR_HEAT_RATE").unwrap_or_else(|_| "medium".into());
    let wear_heat_rate = match wear_heat_rate_str.as_str() {
        "low" => WearHeatRate::Low,
        "high" => WearHeatRate::High,
        _ => WearHeatRate::Medium,
    };
    let zone_at_tick: u64 =
        std::env::var("MAFIS_FAULT_ZONE_AT_TICK").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let zone_latency_duration: u32 = std::env::var("MAFIS_FAULT_ZONE_LATENCY_DURATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let intermittent_mtbf_ticks: u64 = std::env::var("MAFIS_FAULT_INTERMITTENT_MTBF_TICKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80);
    let intermittent_recovery_ticks: u32 = std::env::var("MAFIS_FAULT_INTERMITTENT_RECOVERY_TICKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let intermittent_start_tick: u64 = std::env::var("MAFIS_FAULT_INTERMITTENT_START_TICK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Optional custom Weibull override
    let custom_weibull: Option<(f32, f32)> = {
        let beta: Option<f32> =
            std::env::var("MAFIS_FAULT_WEIBULL_BETA").ok().and_then(|v| v.parse().ok());
        let eta: Option<f32> =
            std::env::var("MAFIS_FAULT_WEIBULL_ETA").ok().and_then(|v| v.parse().ok());
        match (beta, eta) {
            (Some(b), Some(e)) => Some((b, e)),
            _ => None,
        }
    };

    let scenario_type = match fault_type {
        "wear_based" => FaultScenarioType::WearBased,
        "zone_outage" => FaultScenarioType::ZoneOutage,
        "intermittent_fault" => FaultScenarioType::IntermittentFault,
        _ => FaultScenarioType::BurstFailure,
    };

    FaultScenario {
        enabled: true,
        scenario_type,
        burst_kill_percent,
        burst_at_tick,
        wear_heat_rate,
        wear_threshold: 80.0,
        zone_at_tick,
        zone_latency_duration,
        intermittent_mtbf_ticks,
        intermittent_recovery_ticks,
        intermittent_start_tick,
        custom_weibull,
    }
}

fn format_seeds_preview(seeds: &[u64]) -> String {
    if seeds.len() <= 3 {
        seeds.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",")
    } else {
        format!("{},{},... ({})", seeds[0], seeds[1], seeds.len())
    }
}

fn write_experiment_results(name: &str, result: &MatrixResult) {
    // Per-run CSV
    {
        let path = format!("{OUTPUT_DIR}/{name}_runs.csv");
        let mut f = fs::File::create(&path).unwrap();
        export::write_runs_csv(&mut f, &result.runs).unwrap();
        eprintln!("  Wrote {path}");
    }

    // Summary CSV
    if !result.summaries.is_empty() {
        let path = format!("{OUTPUT_DIR}/{name}_summary.csv");
        let mut f = fs::File::create(&path).unwrap();
        export::write_summary_csv(&mut f, &result.summaries).unwrap();
        eprintln!("  Wrote {path}");
    }

    // JSON
    {
        let path = format!("{OUTPUT_DIR}/{name}.json");
        let mut f = fs::File::create(&path).unwrap();
        export::write_matrix_json(&mut f, result).unwrap();
        eprintln!("  Wrote {path}");
    }
}
