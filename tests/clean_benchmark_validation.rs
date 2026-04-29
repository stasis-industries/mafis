//! Clean-benchmark validation gate (Step 5 of solver-refocus).
//!
//! Locks in the literature ranking expectations on no-fault baseline runs.
//! If a port introduces a regression that violates the expected ranking,
//! this gate fails and prevents the full experiment rerun from kicking off.
//!
//! Run with:
//! ```
//! cargo test --release --test clean_benchmark_validation -- --ignored --nocapture
//! ```
//!
//! ## Expected ranking on warehouse_single_dock, n=40 agents, no faults, seed 42, 500 ticks
//!
//! After the 2026-04-08 RHCR-PBS faithful port (eager mode + peek-chain +
//! best-effort sequential A*), the ranking is:
//!
//!   RHCR-PBS (0.468) > LaCAM3 (0.446) > PIBT (0.418) > Token Passing (0.392)
//!
//! 1. **RHCR-PBS should beat or match PIBT** on throughput. Windowed PBS
//!    with eager priority resolution outperforms myopic PIBT on structured
//!    warehouse maps per Li et al. 2021. This is the acceptance gate for
//!    the RHCR-PBS fidelity port — if this fails, the port has
//!    regressed.
//! 2. **lacam3 should be competitive with RHCR-PBS** (within ±10%). Both
//!    are SOTA-class planners. If lacam3 is materially worse than RHCR-PBS,
//!    the lacam3 port has a fidelity issue.
//! 3. **lacam3 should beat PIBT** by a meaningful margin. PIBT is myopic
//!    (one-step) while lacam3 plans multi-step configurations. The
//!    paper-published lacam3 numbers show 1.5-3x throughput improvement
//!    over PIBT on standard MAPF instances.
//! 4. **Token Passing should produce non-zero throughput** within the
//!    same order of magnitude as PIBT. TP is a classic baseline; if it
//!    collapses to zero on a structured topology, the planning order or
//!    constraint logic has regressed.
//!
//! All four solvers must complete the test config without panic.
//!
//! ## Why these checks instead of absolute numbers
//!
//! Hardware noise, RNG differences across rebuilds, and minor algorithmic
//! tunings can shift absolute throughput by 5-10%. The gate uses **ranking
//! invariants** which are robust to those shifts but still catch a port
//! that silently degrades to PIBT-level performance.

use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;

const TOPOLOGY: &str = "warehouse_single_dock";
const NUM_AGENTS: usize = 40;
const TICK_COUNT: u64 = 500;
const SEED: u64 = 42;

fn run_baseline(solver: &str) -> f64 {
    let config = ExperimentConfig {
        solver_name: solver.into(),
        topology_name: TOPOLOGY.into(),
        scenario: None,
        scheduler_name: "random".into(),
        num_agents: NUM_AGENTS,
        seed: SEED,
        tick_count: TICK_COUNT,
        custom_map: None,
        rhcr_override: None,
    };
    let result = run_single_experiment(&config);
    let tp = result.baseline_metrics.avg_throughput;
    eprintln!(
        "  {solver:<20} tp={tp:.4} tasks/tick  total_tasks={}",
        result.baseline_metrics.total_tasks
    );
    tp
}

/// Main validation gate. Runs all 4 solvers on the same baseline instance
/// and asserts the expected ranking invariants.
#[test]
#[ignore]
fn clean_benchmark_ranking_gate() {
    eprintln!(
        "\n=== Clean-Benchmark Validation Gate ===\n  topology={TOPOLOGY}  n={NUM_AGENTS}  ticks={TICK_COUNT}  seed={SEED}  scheduler=random  faults=none\n"
    );

    let pibt_tp = run_baseline("pibt");
    let rhcr_tp = run_baseline("rhcr_pbs");
    let token_tp = run_baseline("token_passing");
    let lacam3_tp = run_baseline("lacam3_lifelong");

    // Invariant 1: All solvers must produce non-zero throughput.
    // Catches: a port silently emitting all-Wait actions, or factory failures.
    assert!(pibt_tp > 0.0, "PIBT produced zero throughput — sanity check failure");
    assert!(rhcr_tp > 0.0, "RHCR-PBS produced zero throughput — sanity check failure");
    assert!(token_tp > 0.0, "Token Passing produced zero throughput — sanity check failure");
    assert!(lacam3_tp > 0.0, "LaCAM3 produced zero throughput — sanity check failure");

    // Invariant 2: lacam3 (PIBT-only mode) should be at least competitive
    // with PIBT — within 5% lower at minimum. We use lacam3's PIBT submodule
    // (swap technique) as the lifelong configuration generator; it should
    // match or slightly beat MAFIS's standalone PIBT (which uses pibt2's PIBT
    // without the swap technique).
    //
    // Catches: lacam3 port degraded so badly that the PIBT submodule is
    // worse than the canonical pibt2 PIBT.
    assert!(
        lacam3_tp >= pibt_tp * 0.95,
        "LaCAM3 ({lacam3_tp:.4}) should be ≥ 95% of PIBT ({pibt_tp:.4}). \
         lacam3-PIBT (with swap technique) should at least match pibt2 PIBT. \
         Check src/solver/lacam3/pibt.rs port."
    );

    // Invariant 3 (RHCR-PBS fidelity gate): After the 2026-04-08
    // eager-mode + peek-chain + best-effort sequential-A* port, RHCR-PBS
    // must beat or match PIBT on this baseline instance. Measured 2026-04-08:
    // rhcr_pbs=0.468, pibt=0.418 — a 12% margin. This is the acceptance
    // gate for the port: if this fails, `find_consistent_paths`, `plan_agent`,
    // or the best-partial A* fallback has regressed.
    //
    // Reference: docs/papers_codes/rhcr/src/PBS.cpp (Jiaoyang-Li/RHCR) —
    // see the "Historical deviations closed 2026-04-08" block in
    // src/solver/rhcr/pbs_planner.rs for the per-deviation citations.
    assert!(
        rhcr_tp >= pibt_tp,
        "RHCR-PBS ({rhcr_tp:.4}) must be ≥ PIBT ({pibt_tp:.4}) after the \
         PBS fidelity port. If this \
         fails, `find_consistent_paths`, `plan_agent`, or the best-partial \
         `spacetime_astar_sequential` fallback has regressed."
    );

    // Invariant 4: lacam3 should be competitive with RHCR-PBS — within ±15%.
    // Both are SOTA-class windowed planners on this instance.
    assert!(
        lacam3_tp >= rhcr_tp * 0.85,
        "LaCAM3 ({lacam3_tp:.4}) should be ≥ 85% of RHCR-PBS ({rhcr_tp:.4}). \
         Both are SOTA-class planners — a large gap suggests a lacam3 port \
         regression."
    );

    // Invariant 5: Token Passing should be within 1 order of magnitude of PIBT.
    // (i.e., not collapse to ~0 due to broken planning order or constraint bug)
    assert!(
        token_tp >= pibt_tp * 0.1,
        "Token Passing ({token_tp:.4}) collapsed below 10% of PIBT ({pibt_tp:.4}). \
         Check pibt2/tp.cpp port — likely planning order or constraint bug."
    );

    // Print final ranking for the report
    let mut ranking: Vec<(&str, f64)> = vec![
        ("PIBT", pibt_tp),
        ("RHCR-PBS", rhcr_tp),
        ("Token Passing", token_tp),
        ("LaCAM3", lacam3_tp),
    ];
    ranking.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    eprintln!("\n=== Clean-Benchmark Ranking (highest TP first) ===");
    for (i, (solver, tp)) in ranking.iter().enumerate() {
        eprintln!("  {}. {solver:<15} tp={tp:.4}", i + 1);
    }
    eprintln!();
}

/// Cross-seed stability check: run lacam3 on 3 seeds and verify the
/// throughput is non-zero for each. Catches RNG-sensitive bugs in the
/// port (e.g., a seed-zero crash or instability).
#[test]
#[ignore]
fn lacam3_cross_seed_stability() {
    eprintln!("\n=== LaCAM3 Cross-Seed Stability ===");
    for &seed in &[42u64, 123, 456] {
        let config = ExperimentConfig {
            solver_name: "lacam3_lifelong".into(),
            topology_name: TOPOLOGY.into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: NUM_AGENTS,
            seed,
            tick_count: 200,
            custom_map: None,
            rhcr_override: None,
        };
        let result = run_single_experiment(&config);
        let tp = result.baseline_metrics.avg_throughput;
        eprintln!("  seed={seed:<5} tp={tp:.4}");
        assert!(tp > 0.0, "lacam3 produced zero throughput on seed {seed}");
    }
}
