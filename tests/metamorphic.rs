//! MET-MAPF Metamorphic Testing — systematic correctness validation.
//!
//! Implements 5 metamorphic relations from the MET-MAPF framework
//! (ACM Transactions on Software Engineering and Methodology, 2024).
//! These tests validate solver correctness WITHOUT needing a ground-truth
//! oracle — they check that properties are preserved under transformations.
//!
//! Run: cargo test --release --test metamorphic -- --nocapture

use std::collections::HashSet;

use bevy::math::IVec2;

use mafis::analysis::baseline::place_agents;
use mafis::core::grid::GridMap;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::topology::{ActiveTopology, ZoneMap, assign_random_zones};
use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::config::FaultConfig;
use mafis::fault::scenario::FaultSchedule;

const SOLVERS: &[&str] = &[
    "pibt", "rhcr_pibt", "token_passing",
    "rt_lacam", "tpts", "pibt+apf",
];

const TICK_COUNT: u64 = 200;

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
        tick_count: TICK_COUNT,
        custom_map: Some((grid, zones)),
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

// ═══════════════════════════════════════════════════════════════════════
// MR1: Agent Removal — removing an agent should not cause deadlock
// ═══════════════════════════════════════════════════════════════════════

/// If a solver produces throughput with N agents, it should also produce
/// throughput with N-1 agents (removing one agent can't cause total failure).
#[test]
fn mr1_agent_removal_preserves_liveness() {
    let (grid, zones) = make_open_grid(16, 16);

    for solver in SOLVERS {
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
            r_full.baseline_metrics.total_tasks,
            r_reduced.baseline_metrics.total_tasks
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
    for solver in SOLVERS {
        // Base: open 16x16 grid
        let (grid_open, zones_open) = make_open_grid(16, 16);
        let r_open = run_custom(solver, grid_open, zones_open, 8, 42);

        // Modified: add 10 obstacles in corners (unlikely to block paths)
        let mut obstacles = HashSet::new();
        for i in 0..5 {
            obstacles.insert(IVec2::new(15, i));     // top-right column
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
            r_open.baseline_metrics.total_tasks,
            r_obs.baseline_metrics.total_tasks
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MR3: Collision freedom is preserved under different seeds
// ═══════════════════════════════════════════════════════════════════════

/// Every solver must produce collision-free plans regardless of seed.
/// Test with 5 different seeds, 15 agents, 200 ticks each.
#[test]
fn mr3_collision_free_across_seeds() {
    let (grid, zones) = make_open_grid(16, 16);

    for solver in SOLVERS {
        for seed in [42, 123, 456, 789, 1024] {
            let grid_area = (grid.width * grid.height) as usize;
            let mut rng = SeededRng::new(seed);
            let agents = place_agents(15, &grid, &zones, &mut rng);

            let solver_box =
                mafis::solver::lifelong_solver_from_name(solver, grid_area, 15)
                    .expect("solver creation failed");
            let scheduler = ActiveScheduler::from_name("random");
            let queue_policy = ActiveQueuePolicy::from_name("closest");

            let mut runner = SimulationRunner::new(
                grid.clone(), zones.clone(), agents, solver_box, rng,
                FaultConfig { enabled: false, ..Default::default() },
                FaultSchedule::default(),
            );

            let mut prev: Vec<IVec2> = runner.agents.iter().map(|a| a.pos).collect();

            for tick in 0..200 {
                runner.tick(scheduler.scheduler(), queue_policy.policy());

                let alive: Vec<IVec2> = runner.agents.iter()
                    .filter(|a| a.alive)
                    .map(|a| a.pos)
                    .collect();
                let unique: HashSet<IVec2> = alive.iter().copied().collect();
                assert_eq!(
                    unique.len(), alive.len(),
                    "MR3: {solver} seed={seed} tick={tick}: vertex collision"
                );

                // Edge swap check
                for i in 0..runner.agents.len() {
                    if !runner.agents[i].alive { continue; }
                    for j in (i+1)..runner.agents.len() {
                        if !runner.agents[j].alive { continue; }
                        if runner.agents[i].pos == prev[j]
                            && runner.agents[j].pos == prev[i]
                            && runner.agents[i].pos != runner.agents[j].pos
                        {
                            panic!("MR3: {solver} seed={seed} tick={tick}: edge swap {i}↔{j}");
                        }
                    }
                }
                prev = runner.agents.iter().map(|a| a.pos).collect();
            }
        }
        eprintln!("  MR3 {solver:<20} 5 seeds x 200 ticks x 15 agents: collision-free");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MR4: Determinism — same input must produce same output
// ═══════════════════════════════════════════════════════════════════════

/// Two runs with identical config must produce byte-identical results.
/// This is the strongest reproducibility test.
#[test]
fn mr4_determinism_on_custom_map() {
    let (grid, zones) = make_open_grid(16, 16);

    for solver in SOLVERS {
        let r1 = run_custom(solver, grid.clone(), zones.clone(), 12, 42);
        let r2 = run_custom(solver, grid.clone(), zones.clone(), 12, 42);

        assert_eq!(
            r1.baseline_metrics.total_tasks,
            r2.baseline_metrics.total_tasks,
            "MR4: {solver} not deterministic: tasks {} vs {}",
            r1.baseline_metrics.total_tasks,
            r2.baseline_metrics.total_tasks
        );
        assert!(
            (r1.baseline_metrics.avg_throughput - r2.baseline_metrics.avg_throughput).abs() < 1e-15,
            "MR4: {solver} throughput diverged"
        );
        eprintln!(
            "  MR4 {solver:<20} deterministic (tasks={})",
            r1.baseline_metrics.total_tasks
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// MR5: Scale monotonicity — more ticks should never reduce total tasks
// ═══════════════════════════════════════════════════════════════════════

/// Running for more ticks should produce at least as many completed tasks.
/// Total tasks at tick 200 >= total tasks at tick 100 (monotonically increasing).
#[test]
fn mr5_more_ticks_more_tasks() {
    let (grid, zones) = make_open_grid(16, 16);

    for solver in SOLVERS {
        let r_short = ExperimentConfig {
            solver_name: solver.to_string(),
            topology_name: "custom".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: Some((grid.clone(), zones.clone())),
        };
        let r_long = ExperimentConfig {
            solver_name: solver.to_string(),
            topology_name: "custom".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 200,
            custom_map: Some((grid.clone(), zones.clone())),
        };

        let result_short = run_single_experiment(&r_short);
        let result_long = run_single_experiment(&r_long);

        assert!(
            result_long.baseline_metrics.total_tasks >= result_short.baseline_metrics.total_tasks,
            "MR5: {solver} fewer tasks with more ticks ({} < {})",
            result_long.baseline_metrics.total_tasks,
            result_short.baseline_metrics.total_tasks
        );
        eprintln!(
            "  MR5 {solver:<20} 100t={:<4} 200t={:<4} OK",
            result_short.baseline_metrics.total_tasks,
            result_long.baseline_metrics.total_tasks
        );
    }
}
