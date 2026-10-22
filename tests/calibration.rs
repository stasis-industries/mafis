//! Self-calibration tests: verify published algorithmic properties.
//!
//! These tests validate that MAFIS solvers exhibit the theoretical properties
//! described in their source papers. This is calibration by property, not by
//! throughput comparison against a reference implementation.
//!
//! Run: cargo test --release --test calibration -- --nocapture

use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::scenario::FaultScenario;
use mafis::planners::DEFAULT;

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
// Property 1: Throughput saturation
//
// Published property (Okumura 2022, Li et al. 2021, Chen et al. 2024):
// In lifelong MAPF on corridor-based maps, throughput increases with agent
// count then saturates or decreases at high density. There exists a density
// where adding agents no longer helps.
//
// Test: run the default planner at increasing densities, verify per-agent throughput
// decreases at high density compared to low density.
// =========================================================================

#[test]
fn property_throughput_saturation() {
    let topology = "warehouse_single_dock"; // 32×21 = 672 cells
    let ticks = 300;

    // Sweep: 5, 10, 20, 40, 60, 80 agents
    let densities = [5, 10, 20, 40, 60, 80];
    let mut throughputs = Vec::new();

    for &n in &densities {
        let t = run_multi_seed(DEFAULT, topology, n, ticks, SEEDS);
        let per_agent = t / n as f64;
        eprintln!("n={n}: throughput={t:.1} tasks ({per_agent:.3} tasks/agent)");
        throughputs.push((t, per_agent));
    }

    // Core saturation property: per-agent throughput must decrease at high
    // density compared to low density. This is the published property
    // (Okumura 2022, Chen et al. 2024): adding agents past the saturation
    // point reduces per-agent efficiency.
    let per_agent_low = throughputs[0].1; // n=5
    let per_agent_high = throughputs[5].1; // n=80
    assert!(
        per_agent_high < per_agent_low,
        "per-agent throughput at n=80 ({per_agent_high:.3}) should be lower than n=5 ({per_agent_low:.3}): saturation expected",
    );

    eprintln!(
        "[OK] Throughput saturation confirmed: per-agent drops from {per_agent_low:.3} (n=5) to {per_agent_high:.3} (n=80)"
    );
}

// =========================================================================
// Property 4: All solvers functional
//
// Verify that every registered planner produces positive throughput and that no
// solver is orders of magnitude worse than the others (which would
// indicate a broken implementation).
//
// Note: relative ordering between solvers depends on density. At low
// density (n=20), Token Passing can outperform PIBT because its
// sequential A* planning produces efficient paths when congestion is
// low. This reverses at high density. We do not assert a specific
// ordering here.
// =========================================================================

#[test]
fn property_solver_paradigm_consistency() {
    let topology = "warehouse_single_dock";
    let ticks = 300;
    let agents = 20;

    let mut results = Vec::new();

    for solver in mafis::planning::registry::entries().iter().map(|entry| entry.info.id) {
        let avg = run_multi_seed(solver, topology, agents, ticks, SEEDS);
        eprintln!("{solver:20}: {avg:.1} tasks");
        assert!(avg > 0.0, "{solver} produced 0 tasks — broken solver");
        results.push((solver, avg));
    }

    // All solvers must produce tasks, and no solver should be more than
    // 100x better than the worst (catches broken implementations).
    let min_t = results.iter().map(|(_, t)| *t).fold(f64::MAX, f64::min);
    let max_t = results.iter().map(|(_, t)| *t).fold(f64::MIN, f64::max);
    assert!(min_t > 0.0, "At least one solver produced 0 tasks");
    assert!(
        max_t < min_t * 100.0,
        "Solver spread too large ({min_t:.1} to {max_t:.1}): likely broken solver"
    );

    eprintln!("[OK] All planners functional: range {min_t:.1} to {max_t:.1} tasks");
}

// =========================================================================
// Property 5: Topology sensitivity
//
// Expected: throughput depends on topology structure. Maps with more
// corridors and chokepoints should produce lower per-agent throughput
// at the same density.
//
// Test: verify that different topologies produce different throughput
// profiles. The tool should be sensitive to map structure.
// =========================================================================

#[test]
fn property_topology_sensitivity() {
    let ticks = 300;
    let agents = 20;

    let topologies = ["warehouse_single_dock", "compact_grid", "sorting_center"];
    let mut throughputs = Vec::new();

    for &topo in &topologies {
        let avg = run_multi_seed(DEFAULT, topo, agents, ticks, SEEDS);
        eprintln!("{topo}: {avg:.1} tasks");
        throughputs.push((topo, avg));
    }

    // At least two topologies should have meaningfully different throughput.
    // If all three produce the same number, the tool isn't measuring topology effects.
    let min_t = throughputs.iter().map(|(_, t)| *t).fold(f64::MAX, f64::min);
    let max_t = throughputs.iter().map(|(_, t)| *t).fold(f64::MIN, f64::max);

    assert!(
        max_t > min_t * 1.1,
        "All topologies produce similar throughput ({min_t:.1} to {max_t:.1}): tool not topology-sensitive"
    );

    eprintln!("[OK] Topology sensitivity confirmed: range {min_t:.1} to {max_t:.1}");
}

// =========================================================================
// Property 6: Differential measurement validity
//
// Core tool property: faulted run should differ from baseline when faults
// are injected. The paired design (same seed) should produce identical
// results when no faults are applied.
//
// Test: run with and without faults on the same seed. Verify that:
// (a) No-fault runs are identical (determinism)
// (b) Faulted runs differ from baseline (faults have measurable effect)
// =========================================================================

#[test]
fn property_differential_measurement_validity() {
    let topology = "compact_grid";
    let agents = 25; // dense on compact_grid (380 walkable) so burst has clear impact
    let ticks = 500; // longer run for more signal

    // Test 1: determinism — two identical runs produce identical results
    for &seed in &[42u64, 123, 456] {
        let baseline_1 = run(DEFAULT, topology, agents, ticks, seed);
        let baseline_2 = run(DEFAULT, topology, agents, ticks, seed);
        assert!(
            (baseline_1 - baseline_2).abs() < 1e-10,
            "Baseline runs differ (seed={seed}): {baseline_1} vs {baseline_2} — determinism broken"
        );
    }
    eprintln!("[OK] Determinism: 3 seeds produce identical paired baselines");

    // Test 2: faults have measurable effect (aggregate across seeds)
    // Use 50% burst kill at tick 50 for a strong, unmissable signal.
    let mut baseline_total = 0.0;
    let mut faulted_total = 0.0;

    for &seed in SEEDS {
        let baseline = run(DEFAULT, topology, agents, ticks, seed);
        baseline_total += baseline;

        let scenario = FaultScenario {
            enabled: true,
            burst_kill_percent: 50.0,
            burst_at_tick: 50,
            ..Default::default()
        };

        let config = ExperimentConfig {
            solver_name: DEFAULT.into(),
            topology_name: topology.into(),
            scenario: Some(scenario),
            scheduler_name: "random".into(),
            num_agents: agents,
            seed,
            tick_count: ticks,
            custom_map: None,
            planner_params: Default::default(),
        };
        let result = run_single_experiment(&config);
        faulted_total += result.faulted_metrics.total_tasks as f64;
    }

    let baseline_avg = baseline_total / SEEDS.len() as f64;
    let faulted_avg = faulted_total / SEEDS.len() as f64;

    eprintln!("Baseline avg: {baseline_avg:.1}, Faulted avg: {faulted_avg:.1}");
    assert!(
        (faulted_avg - baseline_avg).abs() > 1.0,
        "Faulted avg ({faulted_avg:.1}) ≈ baseline avg ({baseline_avg:.1}) — faults have no aggregate effect"
    );

    eprintln!("[OK] Differential measurement valid: faults produce measurable aggregate effect");
}
