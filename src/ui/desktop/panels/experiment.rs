use std::sync::{Arc, Mutex};
use std::thread;

use bevy::prelude::*;
use egui::Color32;

use crate::experiment::config::{ExperimentMatrix, standard_scenarios};
use crate::experiment::export::MetricColumn;
use crate::experiment::runner::{
    ConfigSummary, MatrixResult, RunResult, run_matrix,
    ExperimentProgress as RunnerProgress,
};

use std::sync::atomic::{AtomicBool, Ordering};
use crate::solver::SOLVER_NAMES;
use crate::core::task::SCHEDULER_NAMES;

use crate::core::topology::TopologyRegistry;

/// Presets that can be loaded into the experiment panel.
#[allow(clippy::type_complexity)]
const PRESETS: &[(&str, fn() -> PresetConfig)] = &[
    ("Solver Resilience (75 runs)", preset_solver_resilience),
    ("Scale Sensitivity (100 runs)", preset_scale_sensitivity),
    ("Scheduler Effect (50 runs)", preset_scheduler_effect),
    ("Smoke Test (2 runs)", preset_smoke_test),
];

struct PresetConfig {
    solvers: Vec<String>,
    topologies: Vec<String>,
    schedulers: Vec<String>,
    agent_counts: String,
    seeds: String,
    tick_count: u64,
    use_standard_scenarios: bool,
}

fn preset_solver_resilience() -> PresetConfig {
    PresetConfig {
        solvers: vec!["pibt".into(), "rhcr_pibt".into(), "rhcr_priority_astar".into()],
        topologies: vec!["warehouse_large".into()],
        schedulers: vec!["random".into()],
        agent_counts: "40".into(),
        seeds: "42, 123, 456, 789, 1024".into(),
        tick_count: 500,
        use_standard_scenarios: true,
    }
}

fn preset_scale_sensitivity() -> PresetConfig {
    PresetConfig {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_large".into()],
        schedulers: vec!["random".into()],
        agent_counts: "10, 20, 40, 80".into(),
        seeds: "42, 123, 456, 789, 1024".into(),
        tick_count: 500,
        use_standard_scenarios: true,
    }
}

fn preset_scheduler_effect() -> PresetConfig {
    PresetConfig {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_large".into()],
        schedulers: vec!["random".into(), "closest".into()],
        agent_counts: "40".into(),
        seeds: "42, 123, 456, 789, 1024".into(),
        tick_count: 500,
        use_standard_scenarios: true,
    }
}

fn preset_smoke_test() -> PresetConfig {
    PresetConfig {
        solvers: vec!["pibt".into()],
        topologies: vec!["warehouse_large".into()],
        schedulers: vec!["random".into()],
        agent_counts: "8".into(),
        seeds: "42, 123".into(),
        tick_count: 50,
        use_standard_scenarios: true,
    }
}

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
            solvers: SOLVER_NAMES.iter().enumerate()
                .map(|(i, &(id, _))| (id.to_string(), i == 0))
                .collect(),
            topologies: Vec::new(), // populated from TopologyRegistry at runtime
            schedulers: SCHEDULER_NAMES.iter().enumerate()
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
    fn build_matrix(&self) -> Option<ExperimentMatrix> {
        let solvers: Vec<String> = self.solvers.iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| id.clone())
            .collect();
        let topologies: Vec<String> = self.topologies.iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| id.clone())
            .collect();
        let schedulers: Vec<String> = self.schedulers.iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| id.clone())
            .collect();

        if solvers.is_empty() || topologies.is_empty() || schedulers.is_empty() {
            return None;
        }

        let agent_counts: Vec<usize> = self.agent_counts_text
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .filter(|&n| n > 0)
            .collect();
        let seeds: Vec<u64> = self.seeds_text
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        if agent_counts.is_empty() || seeds.is_empty() {
            return None;
        }

        let scenarios = if self.use_standard_scenarios {
            standard_scenarios()
        } else {
            vec![None]
        };

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

    fn apply_preset(&mut self, preset: &PresetConfig) {
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
    fn sorted_indices(&self, summaries: &[ConfigSummary]) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..summaries.len()).collect();
        let asc = self.sort_ascending;

        indices.sort_by(|&a, &b| {
            let ord = match self.sort_column {
                SortColumn::Solver => summaries[a].solver_name.cmp(&summaries[b].solver_name),
                SortColumn::Topology => summaries[a].topology_name.cmp(&summaries[b].topology_name),
                SortColumn::Scenario => summaries[a].scenario_label.cmp(&summaries[b].scenario_label),
                SortColumn::Scheduler => summaries[a].scheduler_name.cmp(&summaries[b].scheduler_name),
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
// Zone coloring
// ---------------------------------------------------------------------------

fn metric_zone_color(col: MetricColumn, val: f64) -> Color32 {
    match col {
        MetricColumn::FaultTolerance | MetricColumn::Nrr | MetricColumn::SurvivalRate => {
            if val >= 0.7 { Color32::from_rgb(120, 180, 120) }
            else if val >= 0.4 { Color32::from_rgb(200, 170, 100) }
            else { Color32::from_rgb(180, 80, 80) }
        }
        MetricColumn::CriticalTime | MetricColumn::PropagationRate | MetricColumn::ImpactedArea => {
            if val <= 0.2 { Color32::from_rgb(120, 180, 120) }
            else if val <= 0.5 { Color32::from_rgb(200, 170, 100) }
            else { Color32::from_rgb(180, 80, 80) }
        }
        MetricColumn::DeficitRecovery | MetricColumn::ThroughputRecovery => {
            if val <= 20.0 { Color32::from_rgb(120, 180, 120) }
            else if val <= 60.0 { Color32::from_rgb(200, 170, 100) }
            else { Color32::from_rgb(180, 80, 80) }
        }
        MetricColumn::IdleRatio => {
            if val <= 0.3 { Color32::from_rgb(120, 180, 120) }
            else if val <= 0.6 { Color32::from_rgb(200, 170, 100) }
            else { Color32::from_rgb(180, 80, 80) }
        }
        _ => Color32::from_rgb(180, 180, 180), // neutral
    }
}

// ---------------------------------------------------------------------------
// Sortable header helper
// ---------------------------------------------------------------------------

fn sortable_header(
    ui: &mut egui::Ui,
    label: &str,
    col: SortColumn,
    gui_sort: &SortColumn,
    gui_asc: &bool,
) -> bool {
    let arrow = if *gui_sort == col {
        if *gui_asc { " \u{25B2}" } else { " \u{25BC}" }
    } else {
        ""
    };
    let text = format!("{label}{arrow}");
    let resp = ui.add(egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click()));
    resp.clicked()
}

// ---------------------------------------------------------------------------
// Main panel
// ---------------------------------------------------------------------------

/// The metric columns shown in the results table.
const TABLE_METRICS: &[MetricColumn] = &[
    MetricColumn::FaultTolerance,
    MetricColumn::Throughput,
    MetricColumn::Nrr,
    MetricColumn::CriticalTime,
    MetricColumn::SurvivalRate,
    MetricColumn::ThroughputRecovery,
    MetricColumn::PropagationRate,
    MetricColumn::IdleRatio,
    MetricColumn::ImpactedArea,
    MetricColumn::TotalTasks,
    MetricColumn::DeficitIntegral,
    MetricColumn::SolverStepUs,
    MetricColumn::WallTimeMs,
];

/// Populate the experiment topologies list from the registry (once).
fn sync_topologies(gui: &mut ExperimentGuiState, registry: &TopologyRegistry) {
    if gui.topologies.is_empty() && !registry.entries.is_empty() {
        gui.topologies = registry.entries.iter().enumerate()
            .map(|(i, entry)| (entry.id.clone(), i == 0))
            .collect();
    }
}

pub fn experiment_panel(
    ui: &mut egui::Ui,
    gui: &mut ExperimentGuiState,
    handle: Option<&ExperimentHandle>,
    commands: &mut Vec<ExperimentCommand>,
    registry: &TopologyRegistry,
) {
    // Sync topologies from registry if needed
    sync_topologies(gui, registry);

    // Check if running
    let is_done = handle.as_ref().map_or(true, |h| h.done.load(Ordering::Acquire));

    if !is_done {
        let h = handle.unwrap();
        let p = h.progress.lock().unwrap();
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("{}/{} — {}", p.current, p.total, p.label));
        });
        let frac = if p.total > 0 { p.current as f32 / p.total as f32 } else { 0.0 };
        ui.add(egui::ProgressBar::new(frac).show_percentage());
        ui.ctx().request_repaint(); // ensure next frame repaints for progress updates
        return;
    }

    // Check if results just arrived
    if let Some(h) = handle {
        if h.done.load(Ordering::Acquire) {
            let mut result = h.result.lock().unwrap();
            if result.is_some() {
                gui.last_result = result.take();
                gui.selected_row = None;
                gui.show_drill_down = false;
                commands.push(ExperimentCommand::ClearHandle);
            }
        }
    }

    // ── Preset selector ───────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Preset");
        for &(name, factory) in PRESETS {
            if ui.small_button(name).clicked() {
                gui.apply_preset(&factory());
            }
        }
    });

    ui.add_space(4.0);

    // ── Configuration ──────────────────────────────────────────────
    ui.label("Solvers");
    ui.horizontal_wrapped(|ui| {
        for (id, on) in &mut gui.solvers {
            let label = SOLVER_NAMES.iter()
                .find(|(sid, _)| sid == id)
                .map(|(_, l)| *l)
                .unwrap_or(id.as_str());
            let short = label.split('—').next().unwrap_or(label).trim();
            ui.checkbox(on, short);
        }
    });

    ui.label("Topologies");
    ui.horizontal_wrapped(|ui| {
        for (id, on) in &mut gui.topologies {
            ui.checkbox(on, id.as_str());
        }
    });

    ui.label("Schedulers");
    ui.horizontal(|ui| {
        for (id, on) in &mut gui.schedulers {
            ui.checkbox(on, id.as_str());
        }
    });

    ui.checkbox(&mut gui.use_standard_scenarios, "Standard fault scenarios");

    ui.horizontal(|ui| {
        ui.label("Agents");
        ui.text_edit_singleline(&mut gui.agent_counts_text);
    });

    ui.horizontal(|ui| {
        ui.label("Seeds");
        ui.text_edit_singleline(&mut gui.seeds_text);
    });

    ui.horizontal(|ui| {
        ui.label("Ticks");
        let mut t = gui.tick_count as u32;
        if ui.add(egui::DragValue::new(&mut t).range(50..=5000)).changed() {
            gui.tick_count = t as u64;
        }
    });

    // ── Launch + Import ───────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if let Some(matrix) = gui.build_matrix() {
            let total = matrix.total_runs();
            if ui.button(format!("Run ({total} runs)")).clicked() {
                commands.push(ExperimentCommand::Launch(matrix));
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Run (select config)"));
        }

        if ui.button("Import JSON").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Import Experiment Results")
                .add_filter("JSON", &["json"])
                .pick_file()
            {
                match std::fs::read_to_string(&path) {
                    Ok(json) => {
                        match crate::experiment::export::parse_summaries_from_json(&json) {
                            Ok(summaries) => {
                                gui.last_result = Some(MatrixResult {
                                    matrix: ExperimentMatrix {
                                        solvers: vec![],
                                        topologies: vec![],
                                        scenarios: vec![],
                                        schedulers: vec![],
                                        agent_counts: vec![],
                                        seeds: vec![],
                                        tick_count: 0,
                                    },
                                    runs: vec![],
                                    summaries,
                                    wall_time_total_ms: 0,
                                });
                                gui.selected_row = None;
                                gui.show_drill_down = false;
                            }
                            Err(e) => eprintln!("Import error: {e}"),
                        }
                    }
                    Err(e) => eprintln!("File read error: {e}"),
                }
            }
    });

    // ── Results ────────────────────────────────────────────────────
    if gui.last_result.is_none() {
        return;
    }

    ui.add_space(8.0);
    ui.separator();

    // Must extract data before the mutable borrow for sort state
    let (summaries, runs, wall_ms, num_runs) = {
        let r = gui.last_result.as_ref().unwrap();
        (r.summaries.clone(), r.runs.clone(), r.wall_time_total_ms, r.runs.len())
    };

    if num_runs > 0 {
        ui.label(format!(
            "{num_runs} runs in {:.1}s",
            wall_ms as f64 / 1000.0,
        ));
    }
    ui.label(format!("{} configurations", summaries.len()));

    // ── Chart metric selector ─────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Chart metric:");
        egui::ComboBox::from_id_salt("chart_metric")
            .selected_text(gui.chart_metric.label())
            .show_ui(ui, |ui| {
                for &col in TABLE_METRICS {
                    ui.selectable_value(&mut gui.chart_metric, col, col.label());
                }
            });
    });

    // Build sorted indices
    let sorted_indices = gui.sorted_indices(&summaries);

    // ── Bar chart ─────────────────────────────────────────────────
    if summaries.len() > 1 {
        let chart_col = gui.chart_metric;
        let chart_height = (summaries.len() as f32 * 20.0).clamp(60.0, 200.0);

        egui_plot::Plot::new("experiment_bar_chart")
            .height(chart_height)
            .show_axes([true, false])
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .show(ui, |plot_ui| {
                let bars: Vec<egui_plot::Bar> = sorted_indices
                    .iter()
                    .enumerate()
                    .map(|(bar_idx, &data_idx)| {
                        let s = &summaries[data_idx];
                        let stat = chart_col.get_stat(s);
                        let color = metric_zone_color(chart_col, stat.mean);
                        egui_plot::Bar::new(bar_idx as f64, stat.mean)
                            .width(0.7)
                            .name(format!(
                                "{}/{}/{}",
                                s.solver_name, s.scenario_label, s.num_agents
                            ))
                            .fill(egui::Color32::from_rgb(color.r(), color.g(), color.b()))
                    })
                    .collect();
                plot_ui.bar_chart(
                    egui_plot::BarChart::new(chart_col.label(), bars)
                );
            });
    }

    // ── Sortable table ────────────────────────────────────────────
    egui::ScrollArea::horizontal()
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    egui::Grid::new("experiment_results")
                        .striped(true)
                        .min_col_width(40.0)
                        .show(ui, |ui| {
                            // ── Header row ──
                            let config_headers: &[(&str, SortColumn)] = &[
                                ("Solver", SortColumn::Solver),
                                ("Topo", SortColumn::Topology),
                                ("Scenario", SortColumn::Scenario),
                                ("Sched", SortColumn::Scheduler),
                                ("N", SortColumn::Agents),
                            ];

                            for &(label, col) in config_headers {
                                if sortable_header(ui, label, col, &gui.sort_column, &gui.sort_ascending) {
                                    if gui.sort_column == col {
                                        gui.sort_ascending = !gui.sort_ascending;
                                    } else {
                                        gui.sort_column = col;
                                        gui.sort_ascending = true;
                                    }
                                }
                            }

                            for &mcol in TABLE_METRICS {
                                let sc = SortColumn::Metric(mcol);
                                if sortable_header(ui, mcol.short_label(), sc, &gui.sort_column, &gui.sort_ascending) {
                                    if gui.sort_column == sc {
                                        gui.sort_ascending = !gui.sort_ascending;
                                    } else {
                                        gui.sort_column = sc;
                                        gui.sort_ascending = false; // default descending for metrics
                                    }
                                }
                            }
                            ui.end_row();

                            // ── Data rows ──
                            for &idx in &sorted_indices {
                                let s = &summaries[idx];
                                let is_selected = gui.selected_row == Some(idx);

                                // Config columns
                                let row_resp = ui.add(egui::Label::new(&s.solver_name).sense(egui::Sense::click()));
                                if row_resp.clicked() {
                                    gui.selected_row = Some(idx);
                                    gui.show_drill_down = true;
                                }
                                ui.label(&s.topology_name);
                                ui.label(&s.scenario_label);
                                ui.label(&s.scheduler_name);
                                ui.monospace(format!("{}", s.num_agents));

                                // Metric columns with zone coloring + CI hover
                                for &mcol in TABLE_METRICS {
                                    let stat = mcol.get_stat(s);
                                    let d = mcol.decimals();
                                    let color = metric_zone_color(mcol, stat.mean);
                                    let text = format!("{:.prec$}", stat.mean, prec = d);

                                    let label = if is_selected {
                                        egui::RichText::new(&text).color(color).underline()
                                    } else {
                                        egui::RichText::new(&text).color(color)
                                    };

                                    let resp = ui.add(egui::Label::new(label).sense(egui::Sense::hover()));
                                    resp.on_hover_text(format!(
                                        "{}: {:.prec$} ± {:.prec$}\n95% CI: [{:.prec$}, {:.prec$}]\nRange: [{:.prec$}, {:.prec$}]\nn = {}",
                                        mcol.label(),
                                        stat.mean, stat.std,
                                        stat.ci95_lo, stat.ci95_hi,
                                        stat.min, stat.max,
                                        stat.n,
                                        prec = d,
                                    ));
                                }

                                ui.end_row();
                            }
                        });
                });
        });

    // ── Drill-down panel ──────────────────────────────────────────
    if gui.show_drill_down
        && let Some(sel_idx) = gui.selected_row
            && sel_idx < summaries.len() && !runs.is_empty() {
                let sel = &summaries[sel_idx];
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading(format!(
                        "Detail: {} / {} / {} / {} / {} agents",
                        sel.solver_name, sel.topology_name, sel.scenario_label,
                        sel.scheduler_name, sel.num_agents,
                    ));
                    if ui.small_button("Close").clicked() {
                        gui.show_drill_down = false;
                    }
                });

                // Filter runs matching this config
                let matching: Vec<&RunResult> = runs.iter()
                    .filter(|r| {
                        r.config.solver_name == sel.solver_name
                            && r.config.topology_name == sel.topology_name
                            && r.config.scenario_label() == sel.scenario_label
                            && r.config.scheduler_name == sel.scheduler_name
                            && r.config.num_agents == sel.num_agents
                    })
                    .collect();

                if matching.is_empty() {
                    ui.weak("No per-run data available (imported from summaries only)");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .id_salt("drill_down_scroll")
                        .show(ui, |ui| {
                            egui::Grid::new("drill_down_table")
                                .striped(true)
                                .min_col_width(50.0)
                                .show(ui, |ui| {
                                    ui.strong("Seed");
                                    ui.strong("BL TP");
                                    ui.strong("Faulted TP");
                                    ui.strong("FT");
                                    ui.strong("Survival");
                                    ui.strong("MTTR");
                                    ui.strong("Tasks");
                                    ui.end_row();

                                    for run in &matching {
                                        let bl = &run.baseline_metrics;
                                        let f = &run.faulted_metrics;
                                        ui.monospace(format!("{}", run.config.seed));
                                        ui.monospace(format!("{:.2}", bl.avg_throughput));
                                        ui.monospace(format!("{:.2}", f.avg_throughput));
                                        ui.colored_label(
                                            metric_zone_color(MetricColumn::FaultTolerance, f.fault_tolerance),
                                            format!("{:.2}", f.fault_tolerance),
                                        );
                                        ui.colored_label(
                                            metric_zone_color(MetricColumn::SurvivalRate, f.survival_rate),
                                            format!("{:.2}", f.survival_rate),
                                        );
                                        ui.monospace(format!("{:.1}", f.deficit_recovery));
                                        ui.monospace(format!("{}", f.total_tasks));
                                        ui.end_row();
                                    }
                                });
                        });
                }
            }

    // ── Export menu ────────────────────────────────────────────────
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        export_button(ui, "CSV Runs", &summaries, &runs, ExportFormat::CsvRuns);
        export_button(ui, "CSV Summary", &summaries, &runs, ExportFormat::CsvSummary);
        export_button(ui, "JSON", &summaries, &runs, ExportFormat::Json);
        export_button(ui, "LaTeX", &summaries, &runs, ExportFormat::Latex);
        export_button(ui, "Typst", &summaries, &runs, ExportFormat::Typst);
        export_button(ui, "SVG Chart", &summaries, &runs, ExportFormat::Svg);
    });
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum ExportFormat {
    CsvRuns,
    CsvSummary,
    Json,
    Latex,
    Typst,
    Svg,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::CsvRuns | Self::CsvSummary => "csv",
            Self::Json => "json",
            Self::Latex => "tex",
            Self::Typst => "typ",
            Self::Svg => "svg",
        }
    }

    fn filter_name(self) -> &'static str {
        match self {
            Self::CsvRuns | Self::CsvSummary => "CSV",
            Self::Json => "JSON",
            Self::Latex => "LaTeX",
            Self::Typst => "Typst",
            Self::Svg => "SVG",
        }
    }
}

fn export_button(
    ui: &mut egui::Ui,
    label: &str,
    summaries: &[ConfigSummary],
    runs: &[RunResult],
    fmt: ExportFormat,
) {
    if ui.small_button(label).clicked() {
        let ext = fmt.extension();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export")
            .add_filter(fmt.filter_name(), &[ext])
            .save_file()
            && let Err(e) = write_export(&path, summaries, runs, fmt) {
                eprintln!("Export error: {e}");
            }
    }
}

fn write_export(
    path: &std::path::Path,
    summaries: &[ConfigSummary],
    runs: &[RunResult],
    fmt: ExportFormat,
) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| format!("create: {e}"))?;
    use crate::experiment::export;

    match fmt {
        ExportFormat::CsvRuns => {
            export::write_runs_csv(&mut file, runs).map_err(|e| format!("{e}"))?;
        }
        ExportFormat::CsvSummary => {
            export::write_summary_csv(&mut file, summaries).map_err(|e| format!("{e}"))?;
        }
        ExportFormat::Json => {
            // Re-create a MatrixResult for JSON export
            let result = MatrixResult {
                matrix: ExperimentMatrix {
                    solvers: vec![], topologies: vec![], scenarios: vec![],
                    schedulers: vec![], agent_counts: vec![], seeds: vec![],
                    tick_count: 0,
                },
                runs: runs.to_vec(),
                summaries: summaries.to_vec(),
                wall_time_total_ms: 0,
            };
            export::write_matrix_json(&mut file, &result).map_err(|e| format!("{e}"))?;
        }
        ExportFormat::Latex => {
            export::write_latex_table(&mut file, summaries, TABLE_METRICS)
                .map_err(|e| format!("{e}"))?;
        }
        ExportFormat::Typst => {
            export::write_typst_table(&mut file, summaries, TABLE_METRICS)
                .map_err(|e| format!("{e}"))?;
        }
        ExportFormat::Svg => {
            let indices: Vec<usize> = (0..summaries.len()).collect();
            export::write_svg_chart(&mut file, summaries, MetricColumn::FaultTolerance, &indices)
                .map_err(|e| format!("{e}"))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Full-page experiment UI
// ---------------------------------------------------------------------------

/// Renders the full-page experiment view (config → running → results).
pub fn experiment_fullpage_panel(
    ui: &mut egui::Ui,
    gui: &mut ExperimentGuiState,
    handle: Option<&ExperimentHandle>,
    commands: &mut Vec<ExperimentCommand>,
    registry: &TopologyRegistry,
) {
    // Sync topologies from registry if needed
    sync_topologies(gui, registry);
    // Auto-transition: if running and done, go to results
    if gui.stage == ExpStage::Running {
        if let Some(h) = handle {
            if h.done.load(Ordering::Acquire) {
                let mut result = h.result.lock().unwrap();
                if result.is_some() {
                    gui.last_result = result.take();
                    gui.selected_row = None;
                    gui.show_drill_down = false;
                    commands.push(ExperimentCommand::ClearHandle);
                    gui.stage = ExpStage::Results;
                }
            }
        } else if gui.last_result.is_some() {
            // Handle was already consumed by process_experiment_commands —
            // results are ready, just transition to Results stage.
            gui.stage = ExpStage::Results;
        }
    }

    match gui.stage {
        ExpStage::Config => fullpage_config(ui, gui, commands),
        ExpStage::Running => fullpage_running(ui, gui, handle),
        ExpStage::Results => fullpage_results(ui, gui, commands),
    }
}

fn fullpage_config(
    ui: &mut egui::Ui,
    gui: &mut ExperimentGuiState,
    commands: &mut Vec<ExperimentCommand>,
) {
    let avail = ui.available_size();

    egui::ScrollArea::vertical().show(ui, |ui| {
        // Center content
        let max_w = avail.x.min(900.0);
        let pad = ((avail.x - max_w) / 2.0).max(0.0);

        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_max_width(max_w);

                // Title
                ui.horizontal(|ui| {
                    ui.heading("Batch Comparison");
                });

                ui.add_space(12.0);

                // Presets
                ui.horizontal(|ui| {
                    ui.weak("PRESET");
                    ui.add_space(8.0);
                    for &(name, factory) in PRESETS {
                        if ui.small_button(name).clicked() {
                            gui.apply_preset(&factory());
                        }
                    }
                });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                // 2-column layout via columns
                ui.columns(2, |cols| {
                    // Left column: checkboxes
                    let left = &mut cols[0];
                    left.label(egui::RichText::new("SOLVERS").weak().small());
                    left.horizontal_wrapped(|ui| {
                        for (id, on) in &mut gui.solvers {
                            let label = SOLVER_NAMES.iter()
                                .find(|(sid, _)| sid == id)
                                .map(|(_, l)| *l)
                                .unwrap_or(id.as_str());
                            let short = label.split('—').next().unwrap_or(label).trim();
                            ui.checkbox(on, short);
                        }
                    });

                    left.add_space(8.0);
                    left.label(egui::RichText::new("TOPOLOGIES").weak().small());
                    left.horizontal_wrapped(|ui| {
                        for (id, on) in &mut gui.topologies {
                            ui.checkbox(on, id.as_str());
                        }
                    });

                    left.add_space(8.0);
                    left.label(egui::RichText::new("SCHEDULERS").weak().small());
                    left.horizontal(|ui| {
                        for (id, on) in &mut gui.schedulers {
                            ui.checkbox(on, id.as_str());
                        }
                    });

                    left.add_space(8.0);
                    left.label(egui::RichText::new("FAULT SCENARIOS").weak().small());
                    left.checkbox(&mut gui.use_standard_scenarios, "Standard fault scenarios (None, Burst 20%, Wear-Med, Zone 50t)");

                    // Right column: inputs + actions
                    let right = &mut cols[1];
                    right.label(egui::RichText::new("AGENTS").weak().small());
                    right.text_edit_singleline(&mut gui.agent_counts_text);

                    right.add_space(8.0);
                    right.label(egui::RichText::new("SEEDS").weak().small());
                    right.text_edit_singleline(&mut gui.seeds_text);

                    right.add_space(8.0);
                    right.label(egui::RichText::new("TICKS PER RUN").weak().small());
                    let mut t = gui.tick_count as u32;
                    if right.add(egui::DragValue::new(&mut t).range(50..=5000)).changed() {
                        gui.tick_count = t as u64;
                    }

                    right.add_space(16.0);

                    // Matrix breakdown
                    if let Some(matrix) = gui.build_matrix() {
                        let total = matrix.total_runs();
                        let breakdown = format!(
                            "{} solvers × {} topologies × {} schedulers × {} scenarios × {} agent counts × {} seeds",
                            matrix.solvers.len(),
                            matrix.topologies.len(),
                            matrix.schedulers.len(),
                            matrix.scenarios.len(),
                            matrix.agent_counts.len(),
                            matrix.seeds.len(),
                        );
                        right.weak(&breakdown);
                        right.add_space(8.0);

                        // Run count
                        right.heading(format!("{total} runs"));
                        right.add_space(12.0);

                        // Action buttons
                        right.horizontal(|ui| {
                            if ui.button(format!("RUN EXPERIMENT")).clicked() {
                                commands.push(ExperimentCommand::Launch(matrix));
                                gui.stage = ExpStage::Running;
                            }

                            if ui.button("Import JSON").clicked()
                                && let Some(path) = rfd::FileDialog::new()
                                    .set_title("Import Experiment Results")
                                    .add_filter("JSON", &["json"])
                                    .pick_file()
                                {
                                    match std::fs::read_to_string(&path) {
                                        Ok(json) => {
                                            match crate::experiment::export::parse_summaries_from_json(&json) {
                                                Ok(summaries) => {
                                                    gui.last_result = Some(MatrixResult {
                                                        matrix: ExperimentMatrix {
                                                            solvers: vec![], topologies: vec![], scenarios: vec![],
                                                            schedulers: vec![], agent_counts: vec![], seeds: vec![],
                                                            tick_count: 0,
                                                        },
                                                        runs: vec![],
                                                        summaries,
                                                        wall_time_total_ms: 0,
                                                    });
                                                    gui.selected_row = None;
                                                    gui.show_drill_down = false;
                                                    gui.stage = ExpStage::Results;
                                                }
                                                Err(e) => eprintln!("Import error: {e}"),
                                            }
                                        }
                                        Err(e) => eprintln!("File read error: {e}"),
                                    }
                                }
                        });
                    } else {
                        right.add_space(8.0);
                        right.weak("Select at least one option in each category");
                    }
                });
            });
        });
    });
}

fn fullpage_running(
    ui: &mut egui::Ui,
    _gui: &mut ExperimentGuiState,
    handle: Option<&ExperimentHandle>,
) {
    let avail = ui.available_size();

    ui.vertical_centered(|ui| {
        ui.add_space(avail.y * 0.3);

        if let Some(h) = handle {
            let p = h.progress.lock().unwrap();
            let frac = if p.total > 0 { p.current as f32 / p.total as f32 } else { 0.0 };

            ui.heading(format!("{} / {}", p.current, p.total));
            ui.add_space(4.0);
            ui.weak(format!("{:.0}%", frac * 100.0));
            ui.add_space(12.0);

            ui.add(egui::ProgressBar::new(frac).desired_width(400.0));
            ui.add_space(12.0);

            ui.weak(&p.label);

            // ETA estimation
            let elapsed_secs = h.start_time.elapsed().as_secs_f64();
            if p.current > 0 && p.current < p.total {
                let avg_per_run = elapsed_secs / p.current as f64;
                let remaining = (p.total - p.current) as f64 * avg_per_run;
                if remaining < 60.0 {
                    ui.weak(format!("~{:.0}s remaining", remaining));
                } else {
                    ui.weak(format!("~{:.1} min remaining", remaining / 60.0));
                }
            }
            ui.add_space(4.0);
            ui.weak(format!(
                "{} threads  |  {:.1}s elapsed",
                rayon::current_num_threads(),
                elapsed_secs,
            ));
        } else {
            ui.spinner();
            ui.weak("Starting...");
        }

        ui.ctx().request_repaint();
    });
}

fn fullpage_results(
    ui: &mut egui::Ui,
    gui: &mut ExperimentGuiState,
    #[cfg_attr(feature = "headless", allow(unused))]
    commands: &mut Vec<ExperimentCommand>,
) {
    if gui.last_result.is_none() {
        gui.stage = ExpStage::Config;
        return;
    }

    let (summaries, runs, wall_ms, num_runs) = {
        let r = gui.last_result.as_ref().unwrap();
        (r.summaries.clone(), r.runs.clone(), r.wall_time_total_ms, r.runs.len())
    };

    // ── Toolbar strip ──
    ui.horizontal(|ui| {
        ui.strong("RESULTS");
        ui.separator();

        if num_runs > 0 {
            ui.weak(format!(
                "{} configs — {} runs in {:.1}s",
                summaries.len(), num_runs, wall_ms as f64 / 1000.0,
            ));
        } else {
            ui.weak(format!("{} configurations", summaries.len()));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("NEW").clicked() {
                gui.stage = ExpStage::Config;
            }

            ui.separator();

            // Export buttons
            export_button(ui, "SVG", &summaries, &runs, ExportFormat::Svg);
            export_button(ui, "Typst", &summaries, &runs, ExportFormat::Typst);
            export_button(ui, "LaTeX", &summaries, &runs, ExportFormat::Latex);
            export_button(ui, "JSON", &summaries, &runs, ExportFormat::Json);
            export_button(ui, "CSV", &summaries, &runs, ExportFormat::CsvSummary);

            ui.separator();

            // Chart metric selector
            egui::ComboBox::from_id_salt("fullpage_chart_metric")
                .selected_text(gui.chart_metric.label())
                .width(100.0)
                .show_ui(ui, |ui| {
                    for &col in TABLE_METRICS {
                        ui.selectable_value(&mut gui.chart_metric, col, col.label());
                    }
                });
            ui.weak("Chart:");
        });
    });

    ui.separator();

    // Build sorted indices
    let sorted_indices = gui.sorted_indices(&summaries);

    // ── Body: table + chart ──
    let avail = ui.available_size();
    let chart_w = 320.0_f32.min(avail.x * 0.3);

    ui.horizontal(|ui| {
        // Table (takes remaining width)
        ui.vertical(|ui| {
            ui.set_max_width(avail.x - chart_w - 16.0);

            egui::ScrollArea::both()
                .max_height(avail.y - 80.0)
                .show(ui, |ui| {
                    egui::Grid::new("fp_results_table")
                        .striped(true)
                        .min_col_width(32.0)
                        .show(ui, |ui| {
                            // 3D column + config headers + metric headers
                            ui.label(""); // 3D button column
                            let config_headers: &[(&str, SortColumn)] = &[
                                ("Solver", SortColumn::Solver),
                                ("Topo", SortColumn::Topology),
                                ("Scenario", SortColumn::Scenario),
                                ("Sched", SortColumn::Scheduler),
                                ("N", SortColumn::Agents),
                            ];
                            for &(label, col) in config_headers {
                                if sortable_header(ui, label, col, &gui.sort_column, &gui.sort_ascending) {
                                    if gui.sort_column == col {
                                        gui.sort_ascending = !gui.sort_ascending;
                                    } else {
                                        gui.sort_column = col;
                                        gui.sort_ascending = true;
                                    }
                                }
                            }
                            for &mcol in TABLE_METRICS {
                                let sc = SortColumn::Metric(mcol);
                                if sortable_header(ui, mcol.short_label(), sc, &gui.sort_column, &gui.sort_ascending) {
                                    if gui.sort_column == sc {
                                        gui.sort_ascending = !gui.sort_ascending;
                                    } else {
                                        gui.sort_column = sc;
                                        gui.sort_ascending = false;
                                    }
                                }
                            }
                            ui.end_row();

                            // Data rows
                            for &idx in &sorted_indices {
                                let s = &summaries[idx];
                                let is_selected = gui.selected_row == Some(idx);

                                // 3D button — observatory only
                                #[cfg(not(feature = "headless"))]
                                if ui.small_button("3D").clicked() {
                                    commands.push(ExperimentCommand::SimulateIn3D {
                                        solver: s.solver_name.clone(),
                                        topology: s.topology_name.clone(),
                                        scheduler: s.scheduler_name.clone(),
                                        num_agents: s.num_agents,
                                        seed: 42,
                                        tick_count: gui.tick_count,
                                    });
                                }

                                // Config columns
                                let row_resp = ui.add(egui::Label::new(&s.solver_name).sense(egui::Sense::click()));
                                if row_resp.clicked() {
                                    gui.selected_row = Some(idx);
                                    gui.show_drill_down = true;
                                }
                                ui.label(&s.topology_name);
                                ui.label(&s.scenario_label);
                                ui.label(&s.scheduler_name);
                                ui.monospace(format!("{}", s.num_agents));

                                // Metric columns
                                for &mcol in TABLE_METRICS {
                                    let stat = mcol.get_stat(s);
                                    let d = mcol.decimals();
                                    let color = metric_zone_color(mcol, stat.mean);
                                    let text = format!("{:.prec$}", stat.mean, prec = d);
                                    let label = if is_selected {
                                        egui::RichText::new(&text).color(color).underline()
                                    } else {
                                        egui::RichText::new(&text).color(color)
                                    };
                                    let resp = ui.add(egui::Label::new(label).sense(egui::Sense::hover()));
                                    resp.on_hover_text(format!(
                                        "{}: {:.prec$} ± {:.prec$}\n95% CI: [{:.prec$}, {:.prec$}]\nRange: [{:.prec$}, {:.prec$}]\nn = {}",
                                        mcol.label(), stat.mean, stat.std,
                                        stat.ci95_lo, stat.ci95_hi,
                                        stat.min, stat.max, stat.n,
                                        prec = d,
                                    ));
                                }
                                ui.end_row();
                            }
                        });
                });
        });

        ui.separator();

        // Chart sidebar
        ui.vertical(|ui| {
            ui.set_max_width(chart_w);

            if summaries.len() > 1 {
                let chart_col = gui.chart_metric;
                let chart_height = (summaries.len() as f32 * 18.0).clamp(60.0, avail.y - 100.0);

                egui_plot::Plot::new("fp_bar_chart")
                    .height(chart_height)
                    .show_axes([true, false])
                    .allow_drag(false)
                    .allow_zoom(false)
                    .allow_scroll(false)
                    .show(ui, |plot_ui| {
                        let bars: Vec<egui_plot::Bar> = sorted_indices.iter()
                            .enumerate()
                            .map(|(bar_idx, &data_idx)| {
                                let s = &summaries[data_idx];
                                let stat = chart_col.get_stat(s);
                                let color = metric_zone_color(chart_col, stat.mean);
                                egui_plot::Bar::new(bar_idx as f64, stat.mean)
                                    .width(0.7)
                                    .name(format!("{}/{}/{}", s.solver_name, s.scenario_label, s.num_agents))
                                    .fill(color)
                            })
                            .collect();
                        plot_ui.bar_chart(egui_plot::BarChart::new(chart_col.label(), bars));
                    });
            }
        });
    });

    // ── Drill-down ──
    if gui.show_drill_down
        && let Some(sel_idx) = gui.selected_row
            && sel_idx < summaries.len() {
                let sel = &summaries[sel_idx];
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(format!(
                        "{} / {} / {} / {} / {}a",
                        sel.solver_name, sel.topology_name, sel.scenario_label,
                        sel.scheduler_name, sel.num_agents,
                    ));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            gui.show_drill_down = false;
                        }

                        #[cfg(not(feature = "headless"))]
                        if ui.button("SIMULATE IN OBSERVATORY").clicked() {
                            commands.push(ExperimentCommand::SimulateIn3D {
                                solver: sel.solver_name.clone(),
                                topology: sel.topology_name.clone(),
                                scheduler: sel.scheduler_name.clone(),
                                num_agents: sel.num_agents,
                                seed: 42,
                                tick_count: gui.tick_count,
                            });
                        }
                    });
                });

                // Per-seed table
                if !runs.is_empty() {
                    let matching: Vec<&RunResult> = runs.iter()
                        .filter(|r| {
                            r.config.solver_name == sel.solver_name
                                && r.config.topology_name == sel.topology_name
                                && r.config.scenario_label() == sel.scenario_label
                                && r.config.scheduler_name == sel.scheduler_name
                                && r.config.num_agents == sel.num_agents
                        })
                        .collect();

                    if matching.is_empty() {
                        ui.weak("No per-run data (imported from summaries only)");
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .id_salt("fp_drill_down")
                            .show(ui, |ui| {
                                egui::Grid::new("fp_drill_down_table")
                                    .striped(true)
                                    .min_col_width(50.0)
                                    .show(ui, |ui| {
                                        ui.strong("Seed");
                                        ui.strong("BL TP");
                                        ui.strong("Faulted TP");
                                        ui.strong("FT");
                                        ui.strong("Survival");
                                        ui.strong("MTTR");
                                        ui.strong("Tasks");
                                        ui.end_row();

                                        for run in &matching {
                                            let bl = &run.baseline_metrics;
                                            let f = &run.faulted_metrics;
                                            ui.monospace(format!("{}", run.config.seed));
                                            ui.monospace(format!("{:.2}", bl.avg_throughput));
                                            ui.monospace(format!("{:.2}", f.avg_throughput));
                                            ui.colored_label(
                                                metric_zone_color(MetricColumn::FaultTolerance, f.fault_tolerance),
                                                format!("{:.2}", f.fault_tolerance),
                                            );
                                            ui.colored_label(
                                                metric_zone_color(MetricColumn::SurvivalRate, f.survival_rate),
                                                format!("{:.2}", f.survival_rate),
                                            );
                                            ui.monospace(format!("{:.1}", f.deficit_recovery));
                                            ui.monospace(format!("{}", f.total_tasks));
                                            ui.end_row();
                                        }
                                    });
                            });
                    }
                } else {
                    // Summary detail for imported data
                    egui::Grid::new("fp_drill_detail")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Metric");
                            ui.strong("Mean");
                            ui.strong("Std");
                            ui.strong("CI 95%");
                            ui.end_row();

                            for &mcol in TABLE_METRICS {
                                let stat = mcol.get_stat(sel);
                                let d = mcol.decimals();
                                let color = metric_zone_color(mcol, stat.mean);
                                ui.label(mcol.label());
                                ui.colored_label(color, format!("{:.prec$}", stat.mean, prec = d));
                                ui.monospace(format!("{:.prec$}", stat.std, prec = d));
                                ui.monospace(format!("[{:.prec$}, {:.prec$}]", stat.ci95_lo, stat.ci95_hi, prec = d));
                                ui.end_row();
                            }
                        });
                }
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
