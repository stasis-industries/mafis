use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ExportSnapshot {
    pub metadata: ExportMetadata,
    pub config: ExportSimConfig,
    pub agents: Vec<ExportAgent>,
    pub tick_history: Vec<ExportTickRow>,
    pub faults: Vec<ExportFault>,
    pub metrics: ExportMetrics,
    pub heatmap: Vec<ExportHeatmapCell>,
    pub heatmap_traffic: Vec<ExportTrafficCell>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportMetadata {
    pub export_trigger: String,
    pub export_tick: u64,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportSimConfig {
    pub grid_width: i32,
    pub grid_height: i32,
    pub num_agents: usize,
    pub obstacle_density: f32,
    pub obstacle_positions: Vec<[i32; 2]>,
    pub tick_hz: f64,
    pub max_ticks: Option<u64>,
    pub solver_name: String,
    pub solver_optimality: String,
    pub solver_scalability: String,
    pub fault_enabled: bool,
    pub weibull_enabled: bool,
    pub weibull_beta: f32,
    pub weibull_eta: f32,
    pub intermittent_enabled: bool,
    pub intermittent_mtbf_ticks: u64,
    pub intermittent_recovery_ticks: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportAgent {
    pub agent_index: usize,
    pub start_pos: [i32; 2],
    pub goal_pos: [i32; 2],
    pub current_pos: [i32; 2],
    pub is_dead: bool,
    pub heat: f32,
    pub total_moves: u32,
    pub delay_direct: u32,
    pub delay_indirect: u32,
    pub cascade_depth: u32,
    pub idle_ratio: f32,
    pub total_actions: u32,
    pub wait_actions: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportTickRow {
    pub tick: u64,
    pub agent_index: usize,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportFault {
    pub tick: u64,
    pub agent_index: usize,
    pub fault_type: String,
    pub position: [i32; 2],
    pub agents_affected: u32,
    pub cascade_delay: u32,
    pub cascade_depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportMetrics {
    pub aet: f32,
    pub makespan: u64,
    pub mttr: f32,
    pub max_cascade_depth: u32,
    pub total_cascade_cost: u32,
    pub fault_count: u32,
    pub fault_mttr: f32,
    pub recovery_rate: f32,
    pub avg_cascade_spread: f32,
    pub throughput: f32,
    pub idle_ratio: f32,
    pub survival_series: Vec<(u64, f32)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportHeatmapCell {
    pub x: i32,
    pub y: i32,
    pub density: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportTrafficCell {
    pub x: i32,
    pub y: i32,
    pub visit_count: u32,
}
