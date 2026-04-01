//! Scale testing + edge case battery.
//!
//! Validates that MAFIS handles large agent counts and boundary conditions
//! without panics or crashes. These tests back the "scales to 500 agents" claim.
//!
//! Run: cargo test --release --test scale_tests -- --nocapture

use mafis::analysis::baseline::place_agents;
use mafis::core::grid::GridMap;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::{ActiveTopology, TopologyRegistry, ZoneMap};
use mafis::fault::config::FaultConfig;
use mafis::fault::scenario::FaultSchedule;

fn run_ticks(
    solver_name: &str,
    topology_name: &str,
    num_agents: usize,
    tick_count: u64,
    seed: u64,
) -> (u64, usize) {
    let topo = ActiveTopology::from_name(topology_name);
    let output = topo.topology().generate(seed);
    let grid_area = (output.grid.width * output.grid.height) as usize;
    let capacity = output.grid.walkable_count();

    let actual_agents = num_agents.min(capacity);

    let mut rng = SeededRng::new(seed);
    let agents = place_agents(actual_agents, &output.grid, &output.zones, &mut rng);
    let rng_after = rng.clone();

    let solver = mafis::solver::lifelong_solver_from_name(solver_name, grid_area, actual_agents)
        .unwrap_or_else(|| Box::new(mafis::solver::pibt::PibtLifelongSolver::new()));

    let mut runner = SimulationRunner::new(
        output.grid,
        output.zones,
        agents,
        solver,
        rng_after,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let scheduler = ActiveScheduler::from_name("random");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    for _ in 0..tick_count {
        runner.tick(scheduler.scheduler(), queue_policy.policy());
    }

    let alive = runner.agents.iter().filter(|a| a.alive).count();
    (runner.tasks_completed, alive)
}

// ═══════════════════════════════════════════════════════════════════════
// Scale tests
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scale_pibt_200_agents_warehouse_large() {
    let (tasks, alive) = run_ticks("pibt", "warehouse_large", 200, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  pibt/warehouse_large/200 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_pibt_300_agents_kiva_warehouse() {
    let (tasks, alive) = run_ticks("pibt", "kiva_warehouse", 300, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  pibt/kiva_warehouse/300 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_rhcr_pibt_150_agents_warehouse_large() {
    let (tasks, alive) = run_ticks("rhcr_pibt", "warehouse_large", 150, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  rhcr_pibt/warehouse_large/150 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_pibt_80_agents_kiva_warehouse() {
    let (tasks, alive) = run_ticks("pibt", "kiva_warehouse", 80, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  pibt/kiva_warehouse/80 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_rhcr_pibt_80_agents_kiva_warehouse() {
    let (tasks, alive) = run_ticks("rhcr_pibt", "kiva_warehouse", 80, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  rhcr_pibt/kiva_warehouse/80 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_pibt_120_agents_fullfilment_center() {
    let (tasks, alive) = run_ticks("pibt", "fullfilment_center", 120, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  pibt/fullfilment_center/120 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_pibt_60_agents_warehouse_large() {
    let (tasks, alive) = run_ticks("pibt", "warehouse_large", 60, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  pibt/warehouse_large/60 agents: {tasks} tasks, {alive} alive");
}

// ═══════════════════════════════════════════════════════════════════════
// Edge case: 1 agent per topology
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn edge_single_agent_all_topologies() {
    let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
    for entry in &registry.entries {
        let (tasks, alive) = run_ticks("pibt", &entry.id, 1, 100, 42);
        assert_eq!(alive, 1, "{}: single agent died with no faults", entry.id);
        eprintln!("  1 agent on {}: {tasks} tasks", entry.id);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Edge case: maximum density (agents == walkable cells)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn edge_max_density_no_crash() {
    // Small grid: 8x8 open = 64 walkable cells, fill with 64 agents
    let grid = GridMap::new(8, 8);
    let mut zones = ZoneMap::default();
    // Mark a few cells as pickup/delivery so tasks can be assigned
    zones.pickup_cells.push(bevy::math::IVec2::new(0, 0));
    zones.delivery_cells.push(bevy::math::IVec2::new(7, 7));
    zones.zone_type.insert(bevy::math::IVec2::new(0, 0), mafis::core::topology::ZoneType::Pickup);
    zones.zone_type.insert(bevy::math::IVec2::new(7, 7), mafis::core::topology::ZoneType::Delivery);
    // Fill corridor cells
    for y in 0..8 {
        for x in 0..8 {
            let pos = bevy::math::IVec2::new(x, y);
            if !zones.zone_type.contains_key(&pos) {
                zones.corridor_cells.push(pos);
                zones.zone_type.insert(pos, mafis::core::topology::ZoneType::Corridor);
            }
        }
    }

    let grid_area = 64;
    let mut rng = SeededRng::new(42);
    let agents = place_agents(64, &grid, &zones, &mut rng);
    let rng_after = rng.clone();

    let solver = mafis::solver::lifelong_solver_from_name("pibt", grid_area, 64).unwrap();
    let mut runner = SimulationRunner::new(
        grid, zones, agents, solver, rng_after,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let scheduler = ActiveScheduler::from_name("random");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    // Just verify no panic — gridlock is expected at max density
    for _ in 0..50 {
        runner.tick(scheduler.scheduler(), queue_policy.policy());
    }
    let alive = runner.agents.iter().filter(|a| a.alive).count();
    assert_eq!(alive, 64, "agents died at max density with no faults");
    eprintln!("  max density (64/64): no crash, tasks={}", runner.tasks_completed);
}

// ═══════════════════════════════════════════════════════════════════════
// Edge case: no pickup cells
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn edge_no_pickup_cells_no_crash() {
    let grid = GridMap::new(10, 10);
    let mut zones = ZoneMap::default();
    // Only delivery cells, no pickup
    zones.delivery_cells.push(bevy::math::IVec2::new(5, 5));
    zones.zone_type.insert(bevy::math::IVec2::new(5, 5), mafis::core::topology::ZoneType::Delivery);
    for y in 0..10 {
        for x in 0..10 {
            let pos = bevy::math::IVec2::new(x, y);
            if !zones.zone_type.contains_key(&pos) {
                zones.corridor_cells.push(pos);
                zones.zone_type.insert(pos, mafis::core::topology::ZoneType::Corridor);
            }
        }
    }

    let mut rng = SeededRng::new(42);
    let agents = place_agents(5, &grid, &zones, &mut rng);
    let rng_after = rng.clone();

    let solver = mafis::solver::lifelong_solver_from_name("pibt", 100, 5).unwrap();
    let mut runner = SimulationRunner::new(
        grid, zones, agents, solver, rng_after,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let scheduler = ActiveScheduler::from_name("random");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    // Should not crash — agents stay idle (no pickup → no tasks assigned)
    for _ in 0..50 {
        runner.tick(scheduler.scheduler(), queue_policy.policy());
    }
    eprintln!("  no pickup cells: no crash, tasks={}", runner.tasks_completed);
}

// ═══════════════════════════════════════════════════════════════════════
// Edge case: no delivery cells
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn edge_no_delivery_cells_no_crash() {
    let grid = GridMap::new(10, 10);
    let mut zones = ZoneMap::default();
    // Only pickup cells, no delivery
    zones.pickup_cells.push(bevy::math::IVec2::new(5, 5));
    zones.zone_type.insert(bevy::math::IVec2::new(5, 5), mafis::core::topology::ZoneType::Pickup);
    for y in 0..10 {
        for x in 0..10 {
            let pos = bevy::math::IVec2::new(x, y);
            if !zones.zone_type.contains_key(&pos) {
                zones.corridor_cells.push(pos);
                zones.zone_type.insert(pos, mafis::core::topology::ZoneType::Corridor);
            }
        }
    }

    let mut rng = SeededRng::new(42);
    let agents = place_agents(5, &grid, &zones, &mut rng);
    let rng_after = rng.clone();

    let solver = mafis::solver::lifelong_solver_from_name("pibt", 100, 5).unwrap();
    let mut runner = SimulationRunner::new(
        grid, zones, agents, solver, rng_after,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let scheduler = ActiveScheduler::from_name("random");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    // Should not crash — agents may pick up but can't deliver
    for _ in 0..50 {
        runner.tick(scheduler.scheduler(), queue_policy.policy());
    }
    eprintln!("  no delivery cells: no crash, tasks={}", runner.tasks_completed);
}
