//! RHCR in the full simulation loop.

use std::collections::HashSet;

use bevy::math::IVec2;

use crate::analysis::baseline::place_agents;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::SimulationRunner;
use crate::core::seed::SeededRng;
use crate::core::task::ActiveScheduler;
use crate::core::topology::ActiveTopology;
use crate::fault::config::FaultConfig;
use crate::fault::scenario::FaultSchedule;

fn run_ticks(
    solver_name: &str,
    topology_name: &str,
    num_agents: usize,
    tick_count: u64,
    seed: u64,
) -> (u64, usize) {
    let topo = ActiveTopology::from_name(topology_name);
    let output = topo.topology().generate(seed);
    let capacity = output.grid.walkable_count();

    let actual_agents = num_agents.min(capacity);

    let mut rng = SeededRng::new(seed);
    let agents = place_agents(actual_agents, &output.grid, &output.zones, &mut rng);
    let rng_after = rng.clone();

    let solver = crate::planning::registry::build(solver_name, &Default::default(), actual_agents)
        .unwrap_or_else(|e| panic!("{e}"));

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
// A3. RHCR-PBS fallback remains collision-free under dense conditions
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn rhcr_fallback_collision_free() {
    let topo = ActiveTopology::from_name("warehouse_single_dock");
    let output = topo.topology().generate(42);
    let mut rng = SeededRng::new(42);
    let agents = place_agents(40, &output.grid, &output.zones, &mut rng);

    let solver = crate::planning::registry::build("rhcr_pbs", &Default::default(), 40)
        .expect("solver creation failed");
    let scheduler = ActiveScheduler::from_name("random");
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    let mut runner = SimulationRunner::new(
        output.grid,
        output.zones,
        agents,
        solver,
        rng,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let mut prev_positions: Vec<IVec2> = runner.agents.iter().map(|a| a.pos).collect();

    for tick in 0..500 {
        runner.tick(scheduler.scheduler(), queue_policy.policy());

        // Vertex collision check
        let alive_positions: Vec<IVec2> =
            runner.agents.iter().filter(|a| a.alive).map(|a| a.pos).collect();
        let unique: HashSet<IVec2> = alive_positions.iter().copied().collect();
        assert_eq!(
            unique.len(),
            alive_positions.len(),
            "rhcr_pbs(dense) tick {tick}: vertex collision ({} agents, {} unique)",
            alive_positions.len(),
            unique.len()
        );

        // Edge swap check
        for i in 0..runner.agents.len() {
            if !runner.agents[i].alive {
                continue;
            }
            for j in (i + 1)..runner.agents.len() {
                if !runner.agents[j].alive {
                    continue;
                }
                if runner.agents[i].pos == prev_positions[j]
                    && runner.agents[j].pos == prev_positions[i]
                    && runner.agents[i].pos != runner.agents[j].pos
                {
                    panic!("rhcr_pbs(dense) tick {tick}: edge swap between agents {i} and {j}");
                }
            }
        }

        prev_positions = runner.agents.iter().map(|a| a.pos).collect();
    }
    eprintln!("  rhcr_pbs(dense): 500 ticks, 40 agents -- collision-free (fallback exercised)");
}

#[test]
fn scale_rhcr_pbs_150_agents_warehouse_single_dock() {
    let (tasks, alive) = run_ticks("rhcr_pbs", "warehouse_single_dock", 150, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  rhcr_pbs/warehouse_single_dock/150 agents: {tasks} tasks, {alive} alive");
}

#[test]
fn scale_rhcr_pbs_80_agents_warehouse_dual_dock() {
    let (tasks, alive) = run_ticks("rhcr_pbs", "warehouse_dual_dock", 80, 300, 42);
    assert!(alive > 0, "all agents dead with no faults");
    eprintln!("  rhcr_pbs/warehouse_dual_dock/80 agents: {tasks} tasks, {alive} alive");
}
