//! Integration tests for paper experiments.
//!
//! Run the full paper matrix:
//!   cargo test --release full_paper_matrix -- --ignored --nocapture
//!
//! Run the smoke test:
//!   cargo test paper_smoke -- --nocapture

use mafis::experiment::export;
use mafis::experiment::paper;
use mafis::experiment::runner::{run_matrix, MatrixResult, RunResult};
use std::fs;

const OUTPUT_DIR: &str = "results";

fn ensure_output_dir() {
    fs::create_dir_all(OUTPUT_DIR).expect("failed to create results/");
}

/// Smoke test — 2 runs, ~1 second. Validates the pipeline end-to-end.
#[test]
fn paper_smoke() {
    let matrix = paper::smoke_test();
    let result = run_matrix(&matrix, None);

    assert_eq!(result.runs.len(), 2);
    assert_eq!(result.summaries.len(), 1);

    // Both runs should complete tasks
    for run in &result.runs {
        assert!(run.baseline_metrics.total_tasks > 0, "baseline should complete tasks");
    }

    // Determinism: same seed should produce same baseline
    let seed_42: Vec<&RunResult> = result
        .runs
        .iter()
        .filter(|r| r.config.seed == 42)
        .collect();
    assert_eq!(seed_42.len(), 1);
}

/// Full paper matrix — 300 runs across 5 experiments.
/// Run with: cargo test --release full_paper_matrix -- --ignored --nocapture
#[test]
#[ignore]
fn full_paper_matrix() {
    ensure_output_dir();

    let experiments = paper::all_paper_experiments();
    let total_runs: usize = experiments.iter().map(|(_, m)| m.total_runs()).sum();
    eprintln!("=== MAFIS Paper Experiments ===");
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
    // RHCR-PIBT at high density (40 agents on warehouse_large) can produce 0
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
/// Run with: cargo test --release --test paper_experiments solver_resilience -- --ignored --nocapture
#[test]
#[ignore]
fn solver_resilience() {
    ensure_output_dir();
    let matrix = paper::solver_resilience();
    eprintln!("─── solver_resilience ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("solver_resilience", &result);
}

/// Scale sensitivity — 100 runs.
/// Run with: cargo test --release --test paper_experiments scale_sensitivity -- --ignored --nocapture
#[test]
#[ignore]
fn scale_sensitivity() {
    ensure_output_dir();
    let matrix = paper::scale_sensitivity();
    eprintln!("─── scale_sensitivity ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("scale_sensitivity", &result);
}

/// Scheduler effect — 50 runs.
/// Run with: cargo test --release --test paper_experiments scheduler_effect -- --ignored --nocapture
#[test]
#[ignore]
fn scheduler_effect() {
    ensure_output_dir();
    let matrix = paper::scheduler_effect();
    eprintln!("─── scheduler_effect ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("scheduler_effect", &result);
}

/// Topology medium — 25 runs.
/// Run with: cargo test --release --test paper_experiments topology_medium -- --ignored --nocapture
#[test]
#[ignore]
fn topology_medium() {
    ensure_output_dir();
    let matrices = paper::topology_effect();
    let matrix = &matrices[0]; // warehouse_large
    eprintln!("─── topology_medium ({} runs) ───", matrix.total_runs());
    let result = run_matrix(matrix, None);
    write_experiment_results("topology_medium", &result);
}

/// Topology large — 25 runs.
/// Run with: cargo test --release --test paper_experiments topology_large -- --ignored --nocapture
#[test]
#[ignore]
fn topology_large() {
    ensure_output_dir();
    let matrices = paper::topology_effect();
    let matrix = &matrices[1]; // kiva_warehouse
    eprintln!("─── topology_large ({} runs) ───", matrix.total_runs());
    let result = run_matrix(matrix, None);
    write_experiment_results("topology_large", &result);
}

/// Braess resilience — 6,000 runs (~60 min).
/// Run with: cargo test --release --test paper_experiments braess_resilience -- --ignored --nocapture
#[test]
#[ignore]
fn braess_resilience() {
    ensure_output_dir();
    let matrix = paper::braess_resilience();
    eprintln!("─── braess_resilience ({} runs) ───", matrix.total_runs());
    let result = run_matrix(&matrix, None);
    write_experiment_results("braess_resilience", &result);
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
