//! Token Passing in the full simulation loop.

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

// ═══════════════════════════════════════════════════════════════════════
// A2. Token Passing edge-swap audit on compact_grid
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn token_passing_no_edge_swaps() {
    let topo = ActiveTopology::from_name("compact_grid");
    let output = topo.topology().generate(42);
    let mut rng = SeededRng::new(42);
    let agents = place_agents(30, &output.grid, &output.zones, &mut rng);

    let solver = crate::planning::registry::build("token_passing", &Default::default(), 30)
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
            "token_passing/compact_grid tick {tick}: vertex collision ({} agents, {} unique)",
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
                    panic!(
                        "token_passing/compact_grid tick {tick}: edge swap between agents {i} and {j}"
                    );
                }
            }
        }

        prev_positions = runner.agents.iter().map(|a| a.pos).collect();
    }
    eprintln!("  token_passing/compact_grid: 500 ticks, 30 agents -- no collisions");
}

/// Diagnostic: matches the user-reported WASM scenario (SD-w2, Token Passing,
/// 72 agents, closest scheduler) and dumps the worst 10 Loading streaks on
/// the delivery corridor so we can distinguish (a) natural queue-wait,
/// (b) kick-back residual, (c) unknown state-machine bug.
#[test]
#[ignore]
fn diagnose_stuck_tp_sd_w2() {
    use crate::core::task::TaskLeg;

    let topo = ActiveTopology::from_name("warehouse_single_dock_w2");
    let output = topo.topology().generate(42);
    let scheduler = ActiveScheduler::from_name("closest");
    let queue_policy = ActiveQueuePolicy::from_name("closest");
    let agent_count: usize = 72;
    let mut rng_seed = SeededRng::new(42);
    let agents = place_agents(agent_count, &output.grid, &output.zones, &mut rng_seed);
    let rng_after = rng_seed.clone();
    let solver =
        crate::planning::registry::build("token_passing", &Default::default(), agent_count)
            .expect("tp");
    let mut runner = SimulationRunner::new(
        output.grid.clone(),
        output.zones.clone(),
        agents,
        solver,
        rng_after,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    // Delivery cells are at x=55 on SD-w2. Corridor = x∈[51,54]. Also track
    // "at delivery column" = x ∈ [54, 55].
    let in_corridor = |p: IVec2| p.x >= 51 && p.x <= 54;
    let at_delivery = |p: IVec2| p.x >= 54;

    let mut streak_corridor: Vec<u64> = vec![0; agent_count];
    let mut max_corridor: Vec<u64> = vec![0; agent_count];
    let mut streak_delivery: Vec<u64> = vec![0; agent_count];
    let mut max_delivery: Vec<u64> = vec![0; agent_count];
    let mut streak_any_loading: Vec<u64> = vec![0; agent_count];
    let mut max_any_loading: Vec<u64> = vec![0; agent_count];

    // "picking" visual colour = Loading OR TravelEmpty (both amber).
    let is_amber =
        |tl: &TaskLeg| matches!(tl, TaskLeg::Loading(_)) || matches!(tl, TaskLeg::TravelEmpty(_));
    // Track per-agent total AMBER-at-delivery-column tick counts across
    // the whole run — not just consecutive streaks — because a hot-cycling
    // agent can look "stuck" even with 2-tick bursts if it keeps returning.
    let mut total_amber_delivery: Vec<u64> = vec![0; agent_count];

    for _t in 1..=500u64 {
        let _ = runner.tick(scheduler.scheduler(), queue_policy.policy());
        for (i, a) in runner.agents.iter().enumerate() {
            let is_loading = matches!(a.task_leg, TaskLeg::Loading(_));
            if is_loading {
                streak_any_loading[i] += 1;
                max_any_loading[i] = max_any_loading[i].max(streak_any_loading[i]);
            } else {
                streak_any_loading[i] = 0;
            }
            if is_loading && in_corridor(a.pos) {
                streak_corridor[i] += 1;
                max_corridor[i] = max_corridor[i].max(streak_corridor[i]);
            } else {
                streak_corridor[i] = 0;
            }
            if is_loading && at_delivery(a.pos) {
                streak_delivery[i] += 1;
                max_delivery[i] = max_delivery[i].max(streak_delivery[i]);
            } else {
                streak_delivery[i] = 0;
            }
            if is_amber(&a.task_leg) && a.pos.x >= 51 {
                total_amber_delivery[i] += 1;
            }
        }
    }

    let w_any = *max_any_loading.iter().max().unwrap_or(&0);
    let w_cor = *max_corridor.iter().max().unwrap_or(&0);
    let w_del = *max_delivery.iter().max().unwrap_or(&0);
    let total_agent_ticks = 500 * agent_count as u64;
    let total_amber_sum: u64 = total_amber_delivery.iter().sum();
    eprintln!("=== SD-w2 / TP / n=72 / 500 ticks — amber-at-delivery diagnostic ===");
    eprintln!("  worst Loading streak anywhere    : {w_any}");
    eprintln!("  worst Loading streak in corridor : {w_cor}  (x∈[51,54])");
    eprintln!("  worst Loading streak at delivery : {w_del}  (x≥54)");
    eprintln!(
        "  total amber-ticks at delivery area (Loading|TravelEmpty with x≥51) : {total_amber_sum} / {total_agent_ticks}  ({:.2}%)",
        100.0 * total_amber_sum as f64 / total_agent_ticks as f64
    );
    let worst_total = *total_amber_delivery.iter().max().unwrap_or(&0);
    let avg_total = total_amber_sum as f64 / agent_count as f64;
    eprintln!("  worst per-agent amber-ticks at delivery : {worst_total}");
    eprintln!("  avg per-agent amber-ticks at delivery   : {avg_total:.1}");

    // --- Movement / freeze diagnostic: same scenario at user's exact n=85
    // and 800 ticks to catch post-200 freeze pattern --------------------
    eprintln!("\n=== SD-w2 / TP / n=85 / 800 ticks — FREEZE diagnostic ===");
    let topo2 = ActiveTopology::from_name("warehouse_single_dock_w2");
    let output2 = topo2.topology().generate(42);
    let n2: usize = 85;
    let mut rng_seed2 = SeededRng::new(42);
    let agents2 = place_agents(n2, &output2.grid, &output2.zones, &mut rng_seed2);
    let rng_after2 = rng_seed2.clone();
    let solver2 =
        crate::planning::registry::build("token_passing", &Default::default(), n2).expect("tp");
    let mut runner2 = SimulationRunner::new(
        output2.grid.clone(),
        output2.zones.clone(),
        agents2,
        solver2,
        rng_after2,
        FaultConfig { enabled: false, ..Default::default() },
        FaultSchedule::default(),
    );

    let mut prev_pos: Vec<IVec2> = runner2.agents.iter().map(|a| a.pos).collect();
    let mut motionless_streak: Vec<u64> = vec![0; n2];
    let mut max_motionless: Vec<u64> = vec![0; n2];
    let mut tasks_over_time: Vec<u64> = Vec::new();

    for _t in 1..=800u64 {
        let r = runner2.tick(scheduler.scheduler(), queue_policy.policy());
        tasks_over_time.push(r.tasks_completed);
        for (i, a) in runner2.agents.iter().enumerate() {
            if a.pos == prev_pos[i] {
                motionless_streak[i] += 1;
                max_motionless[i] = max_motionless[i].max(motionless_streak[i]);
            } else {
                motionless_streak[i] = 0;
            }
            prev_pos[i] = a.pos;
        }
    }

    let w_frozen = *max_motionless.iter().max().unwrap_or(&0);
    let frozen_count_200 = motionless_streak.iter().filter(|&&s| s >= 200).count();
    let final_tasks = *tasks_over_time.last().unwrap_or(&0);
    let tasks_first_200 = tasks_over_time.get(199).copied().unwrap_or(0);
    let tasks_last_600 = final_tasks.saturating_sub(tasks_first_200);
    eprintln!("  worst motionless streak : {w_frozen} ticks");
    eprintln!("  agents motionless ≥ 200 ticks at end of run : {frozen_count_200} / {n2}");
    eprintln!("  tasks completed in first 200 ticks  : {tasks_first_200}");
    eprintln!("  tasks completed in last 600 ticks   : {tasks_last_600}");
    eprintln!(
        "  throughput collapse ratio (last-600 avg / first-200 avg) : {:.2}x",
        (tasks_last_600 as f64 / 600.0) / (tasks_first_200 as f64 / 200.0).max(1e-9)
    );
    // Dump top 10 most-frozen agents
    let mut idx2: Vec<usize> = (0..n2).collect();
    idx2.sort_by_key(|&i| std::cmp::Reverse(max_motionless[i]));
    eprintln!("  top 10 most-frozen agents:");
    for i in idx2.iter().take(10) {
        let a = &runner2.agents[*i];
        eprintln!(
            "    agent={} max_motionless={} final pos={:?} goal={:?} task_leg={:?}",
            i, max_motionless[*i], a.pos, a.goal, a.task_leg
        );
    }

    // Dump top 10 offenders
    let mut idx: Vec<usize> = (0..agent_count).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(max_any_loading[i]));
    eprintln!("  top 10 Loading streaks:");
    for i in idx.iter().take(10) {
        let a = &runner.agents[*i];
        eprintln!(
            "    agent={} max_any={} max_corridor={} final pos={:?} goal={:?} task_leg={:?}",
            i, max_any_loading[*i], max_corridor[*i], a.pos, a.goal, a.task_leg
        );
    }
}
