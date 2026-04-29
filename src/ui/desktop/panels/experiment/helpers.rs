use egui::Color32;

use crate::experiment::config::ExperimentMatrix;
use crate::experiment::export::MetricColumn;
use crate::experiment::runner::{ConfigSummary, MatrixResult, RunResult};

use super::SortColumn;

// ---------------------------------------------------------------------------
// MatrixResult constructors
// ---------------------------------------------------------------------------

/// Build an empty `ExperimentMatrix` — used when reconstructing `MatrixResult`
/// from imported summaries (no live matrix exists).
fn empty_matrix() -> ExperimentMatrix {
    ExperimentMatrix {
        solvers: vec![],
        topologies: vec![],
        scenarios: vec![],
        schedulers: vec![],
        agent_counts: vec![],
        seeds: vec![],
        tick_count: 0,
        rhcr_overrides: vec![None],
    }
}

/// Build a placeholder `MatrixResult` from imported summaries only —
/// no per-run data, no matrix, zero wall-time. Used by JSON import paths
/// and by the JSON exporter when re-wrapping summaries.
pub fn matrix_result_from_summaries(
    summaries: Vec<ConfigSummary>,
    runs: Vec<RunResult>,
) -> MatrixResult {
    MatrixResult { matrix: empty_matrix(), runs, summaries, wall_time_total_ms: 0 }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Factory returning a fresh `PresetConfig` for the named preset.
pub type PresetFactory = fn() -> PresetConfig;

pub const PRESETS: &[(&str, PresetFactory)] = &[
    ("Solver Resilience (75 runs)", preset_solver_resilience),
    ("Scale Sensitivity (100 runs)", preset_scale_sensitivity),
    ("Scheduler Effect (50 runs)", preset_scheduler_effect),
    ("Smoke Test (2 runs)", preset_smoke_test),
];

pub struct PresetConfig {
    pub solvers: Vec<String>,
    pub topologies: Vec<String>,
    pub schedulers: Vec<String>,
    pub agent_counts: String,
    pub seeds: String,
    pub tick_count: u64,
    pub use_standard_scenarios: bool,
}

fn preset_solver_resilience() -> PresetConfig {
    PresetConfig {
        solvers: vec!["pibt".into(), "rhcr_pbs".into(), "token_passing".into()],
        topologies: vec!["warehouse_single_dock".into()],
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
        topologies: vec!["warehouse_single_dock".into()],
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
        topologies: vec!["warehouse_single_dock".into()],
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
        topologies: vec!["warehouse_single_dock".into()],
        schedulers: vec!["random".into()],
        agent_counts: "8".into(),
        seeds: "42, 123".into(),
        tick_count: 50,
        use_standard_scenarios: true,
    }
}

// ---------------------------------------------------------------------------
// Zone coloring
// ---------------------------------------------------------------------------

pub fn metric_zone_color(col: MetricColumn, val: f64) -> Color32 {
    match col {
        MetricColumn::FaultTolerance | MetricColumn::SurvivalRate => {
            if val >= 0.7 {
                Color32::from_rgb(120, 180, 120)
            } else if val >= 0.4 {
                Color32::from_rgb(200, 170, 100)
            } else {
                Color32::from_rgb(180, 80, 80)
            }
        }
        MetricColumn::CriticalTime | MetricColumn::ImpactedArea => {
            if val <= 0.2 {
                Color32::from_rgb(120, 180, 120)
            } else if val <= 0.5 {
                Color32::from_rgb(200, 170, 100)
            } else {
                Color32::from_rgb(180, 80, 80)
            }
        }
        MetricColumn::DeficitRecovery | MetricColumn::ThroughputRecovery => {
            if val <= 20.0 {
                Color32::from_rgb(120, 180, 120)
            } else if val <= 60.0 {
                Color32::from_rgb(200, 170, 100)
            } else {
                Color32::from_rgb(180, 80, 80)
            }
        }
        MetricColumn::UnassignedRatio => {
            if val <= 0.3 {
                Color32::from_rgb(120, 180, 120)
            } else if val <= 0.6 {
                Color32::from_rgb(200, 170, 100)
            } else {
                Color32::from_rgb(180, 80, 80)
            }
        }
        _ => Color32::from_rgb(180, 180, 180), // neutral
    }
}

// ---------------------------------------------------------------------------
// Sortable header helper
// ---------------------------------------------------------------------------

pub fn sortable_header(
    ui: &mut egui::Ui,
    label: &str,
    col: SortColumn,
    gui_sort: &SortColumn,
    gui_asc: &bool,
) -> bool {
    let arrow =
        if *gui_sort == col { if *gui_asc { " \u{25B2}" } else { " \u{25BC}" } } else { "" };
    let text = format!("{label}{arrow}");
    let resp =
        ui.add(egui::Label::new(egui::RichText::new(text).strong()).sense(egui::Sense::click()));
    resp.clicked()
}

// ---------------------------------------------------------------------------
// Table metric columns
// ---------------------------------------------------------------------------

/// The metric columns shown in the results table.
/// First 6 = primary differential metrics, then context + timing.
pub const TABLE_METRICS: &[MetricColumn] = &[
    MetricColumn::FaultTolerance,
    MetricColumn::CriticalTime,
    MetricColumn::Itae,
    MetricColumn::AttackRate,
    MetricColumn::CascadeDepth,
    MetricColumn::Rapidity,
    MetricColumn::Throughput,
    MetricColumn::SurvivalRate,
    MetricColumn::SolverStepUs,
    MetricColumn::WallTimeMs,
];

// ---------------------------------------------------------------------------
// Topology sync
// ---------------------------------------------------------------------------

/// Populate the experiment topologies list from the registry (once).
pub fn sync_topologies(
    gui: &mut super::ExperimentGuiState,
    registry: &crate::core::topology::TopologyRegistry,
) {
    if gui.topologies.is_empty() && !registry.entries.is_empty() {
        gui.topologies = registry
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.id.clone(), i == 0))
            .collect();
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum ExportFormat {
    CsvRuns,
    CsvSummary,
    Json,
    Latex,
    Typst,
    Svg,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::CsvRuns | Self::CsvSummary => "csv",
            Self::Json => "json",
            Self::Latex => "tex",
            Self::Typst => "typ",
            Self::Svg => "svg",
        }
    }

    pub fn filter_name(self) -> &'static str {
        match self {
            Self::CsvRuns | Self::CsvSummary => "CSV",
            Self::Json => "JSON",
            Self::Latex => "LaTeX",
            Self::Typst => "Typst",
            Self::Svg => "SVG",
        }
    }
}

pub fn export_button(
    ui: &mut egui::Ui,
    label: &str,
    summaries: &[ConfigSummary],
    runs: &[RunResult],
    fmt: ExportFormat,
    chart_metric: MetricColumn,
) {
    if ui.small_button(label).clicked() {
        let ext = fmt.extension();
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export")
            .add_filter(fmt.filter_name(), &[ext])
            .save_file()
            && let Err(e) = write_export(&path, summaries, runs, fmt, chart_metric)
        {
            eprintln!("Export error: {e}");
        }
    }
}

pub fn write_export(
    path: &std::path::Path,
    summaries: &[ConfigSummary],
    runs: &[RunResult],
    fmt: ExportFormat,
    chart_metric: MetricColumn,
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
            // Re-wrap summaries + runs into a MatrixResult for JSON export.
            let result = matrix_result_from_summaries(summaries.to_vec(), runs.to_vec());
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
            export::write_svg_chart(&mut file, summaries, chart_metric, &indices)
                .map_err(|e| format!("{e}"))?;
        }
    }
    Ok(())
}
