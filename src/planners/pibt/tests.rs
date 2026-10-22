//! PIBT in the full simulation loop.

use crate::experiment::config::ExperimentConfig;
use crate::experiment::runner::run_single_experiment;

// ─── Helper ─────────────────────────────────────────────────────────────

fn run(solver: &str, topology: &str, agents: usize, ticks: u64, seed: u64) -> f64 {
    let config = ExperimentConfig {
        solver_name: solver.into(),
        topology_name: topology.into(),
        scenario: None,
        scheduler_name: "random".into(),
        num_agents: agents,
        seed,
        tick_count: ticks,
        custom_map: None,
        planner_params: Default::default(),
    };
    let result = run_single_experiment(&config);
    // No fault → baseline_metrics and faulted_metrics should be identical.
    // Use baseline_metrics for clean throughput.
    result.baseline_metrics.total_tasks as f64
}

fn run_multi_seed(solver: &str, topology: &str, agents: usize, ticks: u64, seeds: &[u64]) -> f64 {
    let sum: f64 = seeds.iter().map(|&s| run(solver, topology, agents, ticks, s)).sum();
    sum / seeds.len() as f64
}

const SEEDS: &[u64] = &[42, 123, 456, 789, 1024];

// =========================================================================
// Property 2: PIBT completeness (liveness on connected graphs)
//
// Published property (Okumura 2022): PIBT is complete for well-formed
// instances. No permanent deadlock on connected graphs. Every agent
// eventually reaches its goal.
//
// Test: run PIBT for enough ticks with moderate density. Verify that
// tasks are completed (throughput > 0) on every topology.
// =========================================================================

#[test]
fn property_pibt_completeness() {
    // Run on all topologies at moderate density
    let topologies =
        ["warehouse_single_dock", "warehouse_dual_dock", "sorting_center", "compact_grid"];
    let ticks = 500;
    let agents = 20;

    for topo in &topologies {
        for &seed in SEEDS {
            let tasks = run("pibt", topo, agents, ticks, seed);
            assert!(
                tasks > 0.0,
                "PIBT on {topo} (seed={seed}) completed 0 tasks — liveness violation"
            );
        }
        let avg = run_multi_seed("pibt", topo, agents, ticks, SEEDS);
        eprintln!("[OK] PIBT completeness on {topo}: avg {avg:.1} tasks in {ticks} ticks");
    }
}

// =========================================================================
// Property 3: PIBT liveness at high density
//
// PIBT should remain live (produce tasks) even at high agent density.
// This tests that the solver does not deadlock on a dense grid.
//
// Note: this does not specifically isolate priority inheritance as the
// mechanism. It verifies the observable consequence: continued task
// completion under congestion.
// =========================================================================

#[test]
fn property_pibt_liveness_at_high_density() {
    let topology = "compact_grid"; // 24x24 grid
    let ticks = 500;

    for &n in &[30, 50, 70] {
        let avg = run_multi_seed("pibt", topology, n, ticks, SEEDS);
        // Minimum throughput threshold: priority inheritance must keep agents
        // moving on dense grids. A correct PIBT implementation achieves 300+
        // tasks at n=30 on compact_grid (26×26). If throughput drops below
        // 100, priority inheritance is broken (e.g., agents can't push blockers).
        let min_tasks = 100.0;
        assert!(
            avg > min_tasks,
            "PIBT produced only {avg:.1} tasks at n={n} on {topology}: \
             expected >{min_tasks} — possible priority inheritance bug"
        );
        let per_agent = avg / n as f64;
        eprintln!("[OK] PIBT n={n} on {topology}: {avg:.1} tasks ({per_agent:.2}/agent)");
    }
}

#[test]
fn pibt_never_needs_the_resolver() {
    let r = run_single_experiment(&ExperimentConfig {
        solver_name: crate::planners::pibt::ID.into(),
        topology_name: "warehouse_single_dock".into(),
        scenario: None,
        scheduler_name: "closest".into(),
        num_agents: 40,
        seed: 42,
        tick_count: 300,
        custom_map: None,
        planner_params: Default::default(),
    });
    assert_eq!(r.baseline_metrics.forced_waits, 0, "pibt: resolver overrode a PIBT move");
}
