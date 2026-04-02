use bevy::prelude::*;

use crate::analysis::cascade::{CascadeState, DelayRecord};
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::heatmap::HeatmapState;
use crate::analysis::metrics::SimMetrics;
use crate::core::agent::{AgentActionStats, AgentRegistry, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::state::SimulationConfig;
use crate::fault::config::FaultConfig;
use crate::fault::heat::HeatState;
use crate::ui::controls::UiState;

use super::FaultLog;
use super::config::ExportTrigger;
use super::data::*;

type AgentQueryItem<'a> = (
    Entity,
    &'a LogicalAgent,
    Option<&'a HeatState>,
    Option<&'a DelayRecord>,
    Option<&'a AgentActionStats>,
    bool, // Has<Dead>
);

pub fn gather_snapshot(
    trigger: &ExportTrigger,
    sim_config: &SimulationConfig,
    grid: &GridMap,
    rng: &SeededRng,
    ui_state: &UiState,
    fault_config: &FaultConfig,
    cascade: &CascadeState,
    metrics: &SimMetrics,
    heatmap: &HeatmapState,
    registry: &AgentRegistry,
    fault_log: &FaultLog,
    fault_metrics: &FaultMetrics,
    topology_name: &str,
    scheduler_name: &str,
    solver_name: &str,
    solver_optimality: &str,
    solver_scalability: &str,
    agent_data: &[AgentQueryItem],
) -> ExportSnapshot {
    let metadata = ExportMetadata {
        mafis_version: crate::constants::VERSION.to_string(),
        export_trigger: trigger.to_string(),
        export_tick: sim_config.tick,
        seed: rng.seed(),
    };

    let mut obstacle_positions: Vec<[i32; 2]> =
        grid.obstacles().iter().map(|p| [p.x, p.y]).collect();
    obstacle_positions.sort();

    let config = ExportSimConfig {
        topology_name: topology_name.to_string(),
        scheduler_name: scheduler_name.to_string(),
        grid_width: grid.width,
        grid_height: grid.height,
        num_agents: ui_state.num_agents,
        obstacle_density: ui_state.obstacle_density,
        obstacle_positions,
        tick_hz: sim_config.tick_hz,
        max_ticks: sim_config.max_ticks,
        solver_name: solver_name.to_string(),
        solver_optimality: solver_optimality.to_string(),
        solver_scalability: solver_scalability.to_string(),
        fault_enabled: fault_config.enabled,
        weibull_enabled: fault_config.weibull_enabled,
        weibull_beta: fault_config.weibull_beta,
        weibull_eta: fault_config.weibull_eta,
        intermittent_enabled: fault_config.intermittent_enabled,
        intermittent_mtbf_ticks: fault_config.intermittent_mtbf_ticks,
        intermittent_recovery_ticks: fault_config.intermittent_recovery_ticks,
    };

    let mut agents: Vec<ExportAgent> = agent_data
        .iter()
        .map(|(entity, agent, heat, delay, action_stats, is_dead)| {
            let agent_index = registry.get_index(*entity).map(|ai| ai.0).unwrap_or(0);

            let (heat_val, moves) = heat.map(|h| (h.heat, h.total_moves)).unwrap_or((0.0, 0));

            let depth = delay.map(|d| d.depth).unwrap_or(0);

            ExportAgent {
                agent_index,
                goal_pos: [agent.goal_pos.x, agent.goal_pos.y],
                current_pos: [agent.current_pos.x, agent.current_pos.y],
                is_dead: *is_dead,
                heat: heat_val,
                total_moves: moves,
                cascade_depth: depth,
                idle_ratio: action_stats.map_or(0.0, |s| s.idle_ratio()),
                total_actions: action_stats.map_or(0, |s| s.total_actions),
                wait_actions: action_stats.map_or(0, |s| s.wait_actions),
            }
        })
        .collect();
    agents.sort_by_key(|a| a.agent_index);

    let faults: Vec<ExportFault> = cascade
        .fault_log
        .iter()
        .map(|entry| {
            let agent_index = registry.get_index(entry.faulted_entity).map(|ai| ai.0).unwrap_or(0);

            let fault_type = fault_log
                .entries
                .iter()
                .find(|fl| fl.entity == entry.faulted_entity && fl.tick == entry.tick)
                .map(|fl| format!("{:?}", fl.fault_type))
                .unwrap_or_else(|| "Unknown".into());

            ExportFault {
                tick: entry.tick,
                agent_index,
                fault_type,
                position: [0, 0],
                agents_affected: entry.agents_affected,
                cascade_delay: entry.agents_affected,
                cascade_depth: entry.max_depth,
            }
        })
        .collect();

    let faults: Vec<ExportFault> = faults
        .into_iter()
        .map(|mut f| {
            if let Some(fl) = fault_log.entries.iter().find(|fl| {
                registry.get_index(fl.entity).map(|ai| ai.0) == Some(f.agent_index)
                    && fl.tick == f.tick
            }) {
                f.position = [fl.position.x, fl.position.y];
            }
            f
        })
        .collect();

    let export_metrics = ExportMetrics {
        aet: metrics.aet,
        makespan: metrics.makespan,
        mttr: metrics.mttr,
        max_cascade_depth: cascade.max_depth,
        total_cascade_cost: cascade.fault_count,
        fault_count: cascade.fault_count,
        fault_mttr: fault_metrics.mttr,
        recovery_rate: fault_metrics.recovery_rate,
        avg_cascade_spread: fault_metrics.avg_cascade_spread,
        throughput: fault_metrics.throughput,
        idle_ratio: fault_metrics.idle_ratio,
        survival_series: fault_metrics.survival_series.iter().copied().collect(),
    };

    let gw = heatmap.grid_w;
    let mut heatmap_cells: Vec<ExportHeatmapCell> = heatmap
        .density
        .iter()
        .enumerate()
        .filter(|(_, d)| **d > 0.0)
        .map(|(i, density)| ExportHeatmapCell {
            x: (i as i32) % gw,
            y: (i as i32) / gw,
            density: *density,
        })
        .collect();
    heatmap_cells.sort_by(|a, b| a.x.cmp(&b.x).then(a.y.cmp(&b.y)));

    let mut traffic_cells: Vec<ExportTrafficCell> = heatmap
        .traffic
        .iter()
        .enumerate()
        .filter(|(_, c)| **c > 0)
        .map(|(i, count)| ExportTrafficCell {
            x: (i as i32) % gw,
            y: (i as i32) / gw,
            visit_count: *count,
        })
        .collect();
    traffic_cells.sort_by(|a, b| a.x.cmp(&b.x).then(a.y.cmp(&b.y)));

    ExportSnapshot {
        metadata,
        config,
        agents,
        faults,
        metrics: export_metrics,
        heatmap: heatmap_cells,
        heatmap_traffic: traffic_cells,
    }
}
