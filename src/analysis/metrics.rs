use bevy::prelude::*;
use std::collections::HashMap;

use crate::core::agent::LogicalAgent;
use crate::core::state::SimulationConfig;
use crate::fault::breakdown::Dead;

use super::cascade::CascadeState;

/// Aggregate metrics for the simulation run.
#[derive(Resource, Debug, Default)]
pub struct SimMetrics {
    /// Average Execution Time: mean ticks from start to goal
    pub aet: f32,
    /// Tick when last agent reaches goal (current tick while running)
    pub makespan: u64,
    /// Mean Time To Recovery estimate
    pub mttr: f32,
    /// Maximum cascade depth across all fault events
    pub max_cascade_depth: u32,
    /// Total cascade cost across all fault events
    pub total_cascade_cost: u32,

    // Internal tracking
    agent_finish_ticks: HashMap<Entity, Option<u64>>,
    finished_count: u32,
    finished_time_sum: u64,
}

impl SimMetrics {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Update metrics every tick.
pub fn update_metrics(
    agents: Query<(Entity, &LogicalAgent), Without<Dead>>,
    sim_config: Res<SimulationConfig>,
    cascade: Res<CascadeState>,
    mut metrics: ResMut<SimMetrics>,
) {
    // Track which agents have newly reached their goal
    for (entity, agent) in &agents {
        let entry = metrics.agent_finish_ticks.entry(entity).or_insert(None);
        if agent.has_reached_goal() && entry.is_none() {
            *entry = Some(sim_config.tick);
            metrics.finished_count += 1;
            metrics.finished_time_sum += sim_config.tick;
        }
    }

    // AET: average of all finished agents' execution times
    if metrics.finished_count > 0 {
        metrics.aet = metrics.finished_time_sum as f32 / metrics.finished_count as f32;
    }

    // Makespan: current tick (finalized when simulation ends)
    metrics.makespan = sim_config.tick;

    // Pull cascade stats
    metrics.max_cascade_depth = cascade.max_depth;
    metrics.total_cascade_cost = cascade.fault_count;
}
