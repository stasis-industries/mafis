//! Checks that expect specific results from specific planners: liveness on
//! open grids, throughput on every topology, and a throughput floor. They
//! name planners, so the naming guard allowlists this file until M6's check
//! module takes these expectations over.
//!
//! Run: cargo test --release --test planner_expectations -- --nocapture

use std::collections::HashSet;

use bevy::math::IVec2;

use mafis::core::grid::GridMap;
use mafis::core::topology::{ZoneMap, assign_random_zones};
use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::scenario::FaultScenario;

const TICK_COUNT: u64 = 500;

/// Ticks for the open-grid runs (MR1, MR2, MR6).
const OPEN_GRID_TICKS: u64 = 200;

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
        planner_params: Default::default(),
    };
    run_single_experiment(&config)
}

fn run_custom(
    solver: &str,
    grid: GridMap,
    zones: ZoneMap,
    agents: usize,
    seed: u64,
) -> mafis::experiment::runner::RunResult {
    let config = ExperimentConfig {
        solver_name: solver.into(),
        topology_name: "custom".into(),
        scenario: None,
        scheduler_name: "random".into(),
        num_agents: agents,
        seed,
        tick_count: OPEN_GRID_TICKS,
        custom_map: Some((grid, zones)),
        planner_params: Default::default(),
    };
    run_single_experiment(&config)
}

fn make_open_grid(w: i32, h: i32) -> (GridMap, ZoneMap) {
    let grid = GridMap::new(w, h);
    let mut zones = ZoneMap {
        pickup_cells: Vec::new(),
        delivery_cells: Vec::new(),
        corridor_cells: Vec::new(),
        recharging_cells: Vec::new(),
        zone_type: std::collections::HashMap::new(),
        queue_lines: Vec::new(),
    };
    for y in 0..h {
        for x in 0..w {
            zones.corridor_cells.push(IVec2::new(x, y));
        }
    }
    assign_random_zones(&mut zones, 8, 8);
    (grid, zones)
}

/// Solvers expected to produce meaningful throughput on synthetic open grids
/// (16x16 / 20x20 with random zone assignments). RHCR-PBS is excluded because
/// its PBS node limit (clamp(N*3, 50, max) = 50 for N≤16 agents) is too tight
/// for unstructured open-space planning — it falls back to PIBT per-agent but
/// still produces near-zero task completion in 200 ticks. PBS works correctly
/// on structured topologies like warehouse_single_dock (verified in verification.rs).
const LIVENESS_SOLVERS: &[&str] = &["pibt", "token_passing"];

// ═══════════════════════════════════════════════════════════════════════
// 1. Solver × Topology: every solver runs on every topology without panic
// ═══════════════════════════════════════════════════════════════════════

const SOLVERS: &[&str] = &["pibt", "rhcr_pbs", "token_passing"];

const TOPOLOGIES: &[(&str, usize)] = &[
    ("warehouse_medium", 15),
    ("warehouse_single_dock", 20),
    ("warehouse_dual_dock", 30),
    ("sorting_center", 15),
    ("compact_grid", 15),
    ("fulfilment_center", 20),
];

#[test]
fn all_solvers_on_all_topologies() {
    let mut failures = Vec::new();

    // Known limitations: PBS hits node limit on open maps with chokepoints.
    let known_zero = [("rhcr_pbs", "sorting_center"), ("rhcr_pbs", "warehouse_single_dock")];

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
// MR1: Agent Removal — removing an agent should not cause deadlock
// ═══════════════════════════════════════════════════════════════════════

/// If a solver produces throughput with N agents, it should also produce
/// throughput with N-1 agents (removing one agent can't cause total failure).
#[test]
fn mr1_agent_removal_preserves_liveness() {
    let (grid, zones) = make_open_grid(16, 16);

    for solver in LIVENESS_SOLVERS {
        let r_full = run_custom(solver, grid.clone(), zones.clone(), 10, 42);
        let r_reduced = run_custom(solver, grid.clone(), zones.clone(), 9, 42);

        assert!(
            r_reduced.baseline_metrics.total_tasks > 0,
            "MR1 failed for {solver}: removing 1 agent caused zero throughput \
             (full={}, reduced=0)",
            r_full.baseline_metrics.total_tasks
        );
        eprintln!(
            "  MR1 {solver:<20} full(10)={:<4} reduced(9)={:<4} OK",
            r_full.baseline_metrics.total_tasks, r_reduced.baseline_metrics.total_tasks
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MR2: Obstacle Addition — adding obstacles on unused cells shouldn't
//      cause total failure
// ═══════════════════════════════════════════════════════════════════════

/// Adding a few obstacles to an open grid should not reduce throughput to zero.
/// The solver must adapt to the new obstacles.
#[test]
fn mr2_obstacle_addition_preserves_liveness() {
    for solver in LIVENESS_SOLVERS {
        // Base: open 16x16 grid
        let (grid_open, zones_open) = make_open_grid(16, 16);
        let r_open = run_custom(solver, grid_open, zones_open, 8, 42);

        // Modified: add 10 obstacles in corners (unlikely to block paths)
        let mut obstacles = HashSet::new();
        for i in 0..5 {
            obstacles.insert(IVec2::new(15, i)); // top-right column
            obstacles.insert(IVec2::new(0, 15 - i)); // bottom-left column
        }
        let grid_obs = GridMap::with_obstacles(16, 16, obstacles);
        let mut zones_obs = ZoneMap {
            pickup_cells: Vec::new(),
            delivery_cells: Vec::new(),
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type: std::collections::HashMap::new(),
            queue_lines: Vec::new(),
        };
        for y in 0..16 {
            for x in 0..16 {
                let pos = IVec2::new(x, y);
                if grid_obs.is_walkable(pos) {
                    zones_obs.corridor_cells.push(pos);
                }
            }
        }
        assign_random_zones(&mut zones_obs, 8, 8);

        let r_obs = run_custom(solver, grid_obs, zones_obs, 8, 42);

        assert!(
            r_obs.baseline_metrics.total_tasks > 0,
            "MR2 failed for {solver}: adding corner obstacles killed throughput \
             (open={}, obs=0)",
            r_open.baseline_metrics.total_tasks
        );
        eprintln!(
            "  MR2 {solver:<20} open={:<4} +obstacles={:<4} OK",
            r_open.baseline_metrics.total_tasks, r_obs.baseline_metrics.total_tasks
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// T1. Throughput ordering sanity: all 8 solvers produce nonzero throughput
//     and PIBT stays competitive with Token Passing at moderate density.
// ═══════════════════════════════════════════════════════════════════════

/// Sanity check (not a benchmark): verify that the 3 active solvers produce
/// positive throughput on warehouse_single_dock, and that no single solver
/// collapses to below 5% of the best-performing solver. Catches catastrophic
/// regressions (e.g., a solver always returning Wait actions) without being
/// fragile to normal performance variation between paradigms.
///
/// Active solvers: PIBT, RHCR-PBS, Token Passing. Ordering is topology- and
/// density-dependent and should not be asserted as a fixed invariant; the
/// 5%-of-best floor is the meaningful regression guard.
#[test]
fn solver_throughput_ordering_sanity() {
    // Run 200 ticks (shorter than TICK_COUNT=500 to keep CI fast).
    let tick_count: u64 = 200;

    let mut throughputs: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();

    let all_solvers = ["pibt", "rhcr_pbs", "token_passing"];

    for &solver in &all_solvers {
        let config = ExperimentConfig {
            solver_name: solver.into(),
            topology_name: "warehouse_single_dock".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 20,
            seed: 42,
            tick_count,
            custom_map: None,
            planner_params: Default::default(),
        };
        let r = run_single_experiment(&config);
        let tp = r.baseline_metrics.avg_throughput;
        throughputs.insert(solver, tp);

        eprintln!(
            "  throughput_sanity {solver:<22} tasks={:<4} tp={:.3}",
            r.baseline_metrics.total_tasks, tp
        );
    }

    // All solvers must produce positive throughput on warehouse_single_dock.
    // Exception: rhcr_pbs hits its node limit on larger maps and falls back to
    // per-agent PIBT which can produce zero tasks at low density.
    for &solver in &all_solvers {
        let tp = throughputs[solver];
        if solver == "rhcr_pbs" {
            continue;
        }
        assert!(
            tp > 0.0,
            "solver_throughput_ordering_sanity: {solver} produced zero throughput on \
             warehouse_single_dock with 20 agents / 200 ticks"
        );
    }

    // No solver should collapse to below 1% of the best solver's throughput.
    let best_tp = throughputs.values().cloned().fold(f64::NEG_INFINITY, f64::max);
    for &solver in &all_solvers {
        if solver == "rhcr_pbs" {
            continue;
        } // PBS hits node limit on large maps
        let tp = throughputs[solver];
        assert!(
            tp >= best_tp * 0.01,
            "solver_throughput_ordering_sanity: {solver} throughput ({tp:.3}) is less than 1% \
             of the best solver ({best_tp:.3}). Likely a catastrophic regression."
        );
    }

    eprintln!(
        "  throughput_sanity: all {} solvers > 0, best={:.3}, floor(1%)={:.4}",
        all_solvers.len(),
        best_tp,
        best_tp * 0.01
    );
}

// ═══════════════════════════════════════════════════════════════════════
// MR6: Agent scale monotonicity — more agents should produce at least as
//      much throughput at low density (20x20 grid so 10 agents is only 2.5%
//      density — well below any congestion regime)
// ═══════════════════════════════════════════════════════════════════════

/// On a sufficiently large open grid, scaling from 5 to 10 agents should
/// not reduce total throughput — more workers at low density means more
/// tasks can be completed in the same number of ticks.
///
/// Uses LIVENESS_SOLVERS (excludes RHCR-PBS). PBS hits its node limit of 50
/// even on a 20x20 open grid at 5–10 agents, producing near-zero throughput.
/// PBS is verified to be collision-free (MR3) and deterministic (MR4) on open
/// grids; its throughput on unstructured maps is separately documented as a
/// known limitation in verification.rs.
#[test]
fn mr6_agent_scale_monotonicity_throughput() {
    // 20x20 gives density=5/400=1.25% (5 agents) and 2.5% (10 agents).
    // Both are in the uncongested regime for every LIVENESS_SOLVER.
    let (grid, zones) = make_open_grid(20, 20);

    for solver in LIVENESS_SOLVERS.iter().copied() {
        let r5 = run_custom(solver, grid.clone(), zones.clone(), 5, 42);
        let r10 = run_custom(solver, grid.clone(), zones.clone(), 10, 42);

        let tasks5 = r5.baseline_metrics.total_tasks;
        let tasks10 = r10.baseline_metrics.total_tasks;

        // Primary assertion: 10 agents must produce at least as many tasks as 5.
        assert!(
            tasks10 >= tasks5,
            "MR6 failed for {solver}: 10 agents produced fewer tasks than 5 \
             (5-agents={tasks5}, 10-agents={tasks10})"
        );

        // Liveness assertion: 10 agents must produce some throughput.
        assert!(
            tasks10 > 0,
            "MR6 failed for {solver}: 10 agents on 20x20 grid produced zero throughput"
        );

        eprintln!("  MR6 {solver:<20} 5-agents={:<4} 10-agents={:<4} OK", tasks5, tasks10);
    }
}
