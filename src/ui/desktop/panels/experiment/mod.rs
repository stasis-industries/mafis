use std::sync::{Arc, Mutex};
use std::thread;

use bevy::prelude::*;

use crate::experiment::config::ExperimentMatrix;
use crate::experiment::export::MetricColumn;
use crate::experiment::runner::{ExperimentProgress as RunnerProgress, MatrixResult, run_matrix};

use crate::core::task::SCHEDULER_NAMES;
use crate::solver::SOLVER_NAMES;
use std::sync::atomic::{AtomicBool, Ordering};

mod compact;
mod fullpage;
mod helpers;

pub use compact::experiment_panel;
pub use fullpage::experiment_fullpage_panel;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Background experiment handle — stored as a Bevy resource.
///
/// The runner's `RunnerProgress` is updated directly by rayon workers.
/// The UI reads it each frame without a sync thread.
#[derive(Resource)]
pub struct ExperimentHandle {
    pub progress: Arc<Mutex<RunnerProgress>>,
    pub done: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<MatrixResult>>>,
    pub start_time: std::time::Instant,
}

/// Stage for full-page experiment mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpStage {
    Config,
    Running,
    Results,
}

/// GUI state for the experiment panel.
#[derive(Resource)]
pub struct ExperimentGuiState {
    // Selection checkboxes
    pub solvers: Vec<(String, bool)>,
    pub topologies: Vec<(String, bool)>,
    pub schedulers: Vec<(String, bool)>,
    pub use_standard_scenarios: bool,

    // Parameters
    pub agent_counts_text: String,
    pub seeds_text: String,
    pub tick_count: u64,

    // Results
    pub last_result: Option<MatrixResult>,

    // Table state
    pub sort_column: SortColumn,
    pub sort_ascending: bool,
    pub selected_row: Option<usize>,
    pub chart_metric: MetricColumn,
    pub show_drill_down: bool,

    // Full-page stage
    pub stage: ExpStage,
}

/// Columns available for sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Solver,
    Topology,
    Scenario,
    Scheduler,
    Agents,
    Metric(MetricColumn),
}

impl Default for ExperimentGuiState {
    fn default() -> Self {
        Self {
            solvers: SOLVER_NAMES
                .iter()
                .enumerate()
                .map(|(i, &(id, _))| (id.to_string(), i == 0))
                .collect(),
            topologies: Vec::new(), // populated from TopologyRegistry at runtime
            schedulers: SCHEDULER_NAMES
                .iter()
                .enumerate()
                .map(|(i, &(id, _))| (id.to_string(), i == 0))
                .collect(),
            use_standard_scenarios: true,
            agent_counts_text: "20, 40, 80".to_string(),
            seeds_text: "42, 123, 456".to_string(),
            tick_count: 500,
            last_result: None,
            sort_column: SortColumn::Metric(MetricColumn::FaultTolerance),
            sort_ascending: false,
            selected_row: None,
            chart_metric: MetricColumn::FaultTolerance,
            show_drill_down: false,
            stage: ExpStage::Config,
        }
    }
}

impl ExperimentGuiState {
    pub(super) fn build_matrix(&self) -> Option<ExperimentMatrix> {
        use crate::experiment::config::standard_scenarios;

        let solvers: Vec<String> =
            self.solvers.iter().filter(|(_, on)| *on).map(|(id, _)| id.clone()).collect();
        let topologies: Vec<String> =
            self.topologies.iter().filter(|(_, on)| *on).map(|(id, _)| id.clone()).collect();
        let schedulers: Vec<String> =
            self.schedulers.iter().filter(|(_, on)| *on).map(|(id, _)| id.clone()).collect();

        if solvers.is_empty() || topologies.is_empty() || schedulers.is_empty() {
            return None;
        }

        let agent_counts: Vec<usize> = self
            .agent_counts_text
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .filter(|&n| n > 0)
            .collect();
        let seeds: Vec<u64> =
            self.seeds_text.split(',').filter_map(|s| s.trim().parse().ok()).collect();

        if agent_counts.is_empty() || seeds.is_empty() {
            return None;
        }

        let scenarios = if self.use_standard_scenarios { standard_scenarios() } else { vec![None] };

        Some(ExperimentMatrix {
            solvers,
            topologies,
            scenarios,
            schedulers,
            agent_counts,
            seeds,
            tick_count: self.tick_count,
        })
    }

    pub(super) fn apply_preset(&mut self, preset: &helpers::PresetConfig) {
        for (id, on) in &mut self.solvers {
            *on = preset.solvers.contains(id);
        }
        for (id, on) in &mut self.topologies {
            *on = preset.topologies.contains(id);
        }
        for (id, on) in &mut self.schedulers {
            *on = preset.schedulers.contains(id);
        }
        self.agent_counts_text = preset.agent_counts.clone();
        self.seeds_text = preset.seeds.clone();
        self.tick_count = preset.tick_count;
        self.use_standard_scenarios = preset.use_standard_scenarios;
    }

    /// Build sorted indices for the summary table.
    pub(super) fn sorted_indices(
        &self,
        summaries: &[crate::experiment::runner::ConfigSummary],
    ) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..summaries.len()).collect();
        let asc = self.sort_ascending;

        indices.sort_by(|&a, &b| {
            let ord = match self.sort_column {
                SortColumn::Solver => summaries[a].solver_name.cmp(&summaries[b].solver_name),
                SortColumn::Topology => summaries[a].topology_name.cmp(&summaries[b].topology_name),
                SortColumn::Scenario => {
                    summaries[a].scenario_label.cmp(&summaries[b].scenario_label)
                }
                SortColumn::Scheduler => {
                    summaries[a].scheduler_name.cmp(&summaries[b].scheduler_name)
                }
                SortColumn::Agents => summaries[a].num_agents.cmp(&summaries[b].num_agents),
                SortColumn::Metric(col) => {
                    let va = col.get_stat(&summaries[a]).mean;
                    let vb = col.get_stat(&summaries[b]).mean;
                    va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
                }
            };
            if asc { ord } else { ord.reverse() }
        });

        indices
    }
}

// ---------------------------------------------------------------------------
// Commands + launch
// ---------------------------------------------------------------------------

/// Commands generated by the experiment panel, processed by the system.
pub enum ExperimentCommand {
    Launch(ExperimentMatrix),
    ClearHandle,
    /// Pre-configure the simulator with a specific experiment config.
    /// Observatory only — not available in headless experiment mode.
    #[cfg(not(feature = "headless"))]
    SimulateIn3D {
        solver: String,
        topology: String,
        scheduler: String,
        num_agents: usize,
        seed: u64,
        tick_count: u64,
    },
}

/// Launches the experiment in a background thread.
///
/// Uses a shared RunnerProgress that rayon workers update directly.
/// The UI polls the same Arc each frame — no sync thread needed.
pub fn launch_experiment(matrix: ExperimentMatrix) -> ExperimentHandle {
    let runner_progress = Arc::new(Mutex::new(RunnerProgress {
        current: 0,
        total: matrix.total_runs(),
        label: "Starting...".to_string(),
    }));

    let done = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));

    let rp = runner_progress.clone();
    let d = done.clone();
    let r = result.clone();

    thread::spawn(move || {
        let res = run_matrix(&matrix, Some(&rp));
        *r.lock().unwrap() = Some(res);
        if let Ok(mut p) = rp.lock() {
            p.label = "Complete".to_string();
        }
        d.store(true, Ordering::Release);
    });

    ExperimentHandle {
        progress: runner_progress,
        done,
        result,
        start_time: std::time::Instant::now(),
    }
}
