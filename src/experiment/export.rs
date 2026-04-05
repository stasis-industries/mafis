//! CSV and JSON export for experiment results.

use std::io::Write;

use super::runner::{ConfigSummary, MatrixResult, RunResult};

/// Write per-run CSV to a writer.
pub fn write_runs_csv<W: Write>(writer: &mut W, runs: &[RunResult]) -> std::io::Result<()> {
    // Header
    writeln!(
        writer,
        "solver,topology,scenario,scheduler,num_agents,seed,is_baseline,\
         avg_throughput,total_tasks,unassigned_ratio,wait_ratio,\
         fault_tolerance,critical_time,\
         deficit_recovery,throughput_recovery,mtbf,recovery_tick,\
         survival_rate,impacted_area,deficit_integral,\
         cascade_depth_avg,cascade_spread_avg,fleet_utilization,\
         solver_step_avg_us,solver_step_max_us,wall_time_ms"
    )?;

    for run in runs {
        // Baseline row
        write_run_row(writer, &run.config, true, &run.baseline_metrics)?;
        // Faulted row
        write_run_row(writer, &run.config, false, &run.faulted_metrics)?;
    }

    Ok(())
}

/// Format an f64 for CSV: NaN → empty string, finite → formatted number.
fn csv_f64(v: f64, precision: usize) -> String {
    if v.is_nan() { String::new() } else { format!("{:.prec$}", v, prec = precision) }
}

fn write_run_row<W: Write>(
    writer: &mut W,
    config: &super::config::ExperimentConfig,
    is_baseline: bool,
    m: &super::metrics::RunMetrics,
) -> std::io::Result<()> {
    writeln!(
        writer,
        "{},{},{},{},{},{},{},\
         {},{},{},{},\
         {},{},\
         {},{},{},{},\
         {},{},{},\
         {},{},{},\
         {},{},{}",
        config.solver_name,
        config.topology_name,
        config.scenario_label(),
        config.scheduler_name,
        config.num_agents,
        config.seed,
        if is_baseline { "true" } else { "false" },
        csv_f64(m.avg_throughput, 4),
        m.total_tasks,
        csv_f64(m.unassigned_ratio, 4),
        csv_f64(m.wait_ratio, 4),
        csv_f64(m.fault_tolerance, 4),
        csv_f64(m.critical_time, 4),
        csv_f64(m.deficit_recovery, 2),
        csv_f64(m.throughput_recovery, 2),
        m.mtbf.map_or("".to_string(), |v| csv_f64(v, 2)),
        m.recovery_tick.map_or("".to_string(), |v| v.to_string()),
        csv_f64(m.survival_rate, 4),
        csv_f64(m.impacted_area, 4),
        m.deficit_integral,
        csv_f64(m.cascade_depth_avg, 2),
        csv_f64(m.cascade_spread_avg, 2),
        csv_f64(m.fleet_utilization, 4),
        csv_f64(m.solver_step_time_avg_us, 1),
        csv_f64(m.solver_step_time_max_us, 1),
        m.wall_time_ms,
    )
}

/// Write summary CSV (one row per config, across seeds).
pub fn write_summary_csv<W: Write>(
    writer: &mut W,
    summaries: &[ConfigSummary],
) -> std::io::Result<()> {
    // Per-metric `n` columns expose when NaN filtering reduced sample size.
    // ft_n < num_seeds means some runs had baseline throughput = 0.
    // ct_n < num_seeds means some runs had no fault impact (critical_time undefined).
    writeln!(
        writer,
        "solver,topology,scenario,scheduler,num_agents,num_seeds,\
         throughput_mean,throughput_std,throughput_ci95_lo,throughput_ci95_hi,\
         tasks_mean,tasks_std,\
         ft_n,ft_mean,ft_std,ft_ci95_lo,ft_ci95_hi,\
         ct_n,critical_time_mean,critical_time_std,\
         deficit_recovery_mean,deficit_recovery_std,\
         throughput_recovery_mean,throughput_recovery_std,\
         survival_rate_mean,\
         impacted_area_mean,deficit_integral_mean,\
         cascade_depth_mean,cascade_depth_std,cascade_spread_mean,cascade_spread_std,\
         fleet_utilization_mean,fleet_utilization_std,\
         solver_step_us_mean,wall_time_ms_mean"
    )?;

    for s in summaries {
        writeln!(
            writer,
            "{},{},{},{},{},{},\
             {:.4},{:.4},{:.4},{:.4},\
             {:.1},{:.1},\
             {},{:.4},{:.4},{:.4},{:.4},\
             {},{:.4},{:.4},\
             {:.2},{:.2},\
             {:.2},{:.2},\
             {:.4},\
             {:.4},{:.1},\
             {:.2},{:.2},{:.2},{:.2},\
             {:.4},{:.4},\
             {:.1},{:.0}",
            s.solver_name,
            s.topology_name,
            s.scenario_label,
            s.scheduler_name,
            s.num_agents,
            s.num_seeds,
            s.throughput.mean,
            s.throughput.std,
            s.throughput.ci95_lo,
            s.throughput.ci95_hi,
            s.total_tasks.mean,
            s.total_tasks.std,
            s.fault_tolerance.n,
            s.fault_tolerance.mean,
            s.fault_tolerance.std,
            s.fault_tolerance.ci95_lo,
            s.fault_tolerance.ci95_hi,
            s.critical_time.n,
            s.critical_time.mean,
            s.critical_time.std,
            s.deficit_recovery.mean,
            s.deficit_recovery.std,
            s.throughput_recovery.mean,
            s.throughput_recovery.std,
            s.survival_rate.mean,
            s.impacted_area.mean,
            s.deficit_integral.mean,
            s.cascade_depth.mean,
            s.cascade_depth.std,
            s.cascade_spread.mean,
            s.cascade_spread.std,
            s.fleet_utilization.mean,
            s.fleet_utilization.std,
            s.solver_step_us.mean,
            s.wall_time_ms.mean,
        )?;
    }

    Ok(())
}

/// Build the deduplication key for a run's baseline.
/// Configs sharing `(solver, topology, scheduler, num_agents, seed)` produce identical baselines.
fn baseline_key_str(config: &super::config::ExperimentConfig) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        config.solver_name,
        config.topology_name,
        config.scheduler_name,
        config.num_agents,
        config.seed,
    )
}

/// Serialize the full matrix result to JSON.
///
/// Baselines are hoisted into a top-level `baselines` map keyed by
/// `"solver|topology|scheduler|num_agents|seed"`. Each run references its
/// baseline via `baseline_key` instead of embedding a full copy, eliminating
/// N-1 duplicates per scenario group (e.g. 6× savings for a 7-scenario matrix).
pub fn write_matrix_json<W: Write>(writer: &mut W, result: &MatrixResult) -> std::io::Result<()> {
    // Manual JSON serialization to avoid serde dependency in this module.
    // Keeps it lightweight and self-contained.
    write!(writer, "{{")?;

    // Top-level metadata
    write!(
        writer,
        "\"total_runs\":{},\"wall_time_total_ms\":{},",
        result.runs.len(),
        result.wall_time_total_ms
    )?;

    // Collect unique baselines in order of first appearance.
    // Uses HashMap for O(1) dedup; Vec preserves insertion order for deterministic output.
    let mut baseline_order: Vec<String> = Vec::new();
    let mut baseline_map: std::collections::HashMap<String, &super::metrics::RunMetrics> =
        std::collections::HashMap::new();
    for run in &result.runs {
        let key = baseline_key_str(&run.config);
        if let std::collections::hash_map::Entry::Vacant(e) = baseline_map.entry(key.clone()) {
            baseline_order.push(key);
            e.insert(&run.baseline_metrics);
        }
    }

    // Write deduplicated baselines section
    write!(writer, "\"baselines\":{{")?;
    for (i, key) in baseline_order.iter().enumerate() {
        if i > 0 {
            write!(writer, ",")?;
        }
        write!(writer, "\"{}\":", key)?;
        write_metrics_json(writer, baseline_map[key])?;
    }
    write!(writer, "}},")?;

    // Runs array — reference baseline by key instead of embedding a full copy
    write!(writer, "\"runs\":[")?;
    for (i, run) in result.runs.iter().enumerate() {
        if i > 0 {
            write!(writer, ",")?;
        }
        write_run_json(writer, run)?;
    }
    write!(writer, "],")?;

    // Summaries array
    write!(writer, "\"summaries\":[")?;
    for (i, summary) in result.summaries.iter().enumerate() {
        if i > 0 {
            write!(writer, ",")?;
        }
        write_summary_json(writer, summary)?;
    }
    write!(writer, "]")?;

    writeln!(writer, "}}")?;
    Ok(())
}

fn write_run_json<W: Write>(writer: &mut W, run: &RunResult) -> std::io::Result<()> {
    write!(
        writer,
        "{{\"config\":{{\"solver\":\"{}\",\"topology\":\"{}\",\"scenario\":\"{}\",\
         \"scheduler\":\"{}\",\"num_agents\":{},\"seed\":{},\"tick_count\":{}}},",
        run.config.solver_name,
        run.config.topology_name,
        run.config.scenario_label(),
        run.config.scheduler_name,
        run.config.num_agents,
        run.config.seed,
        run.config.tick_count,
    )?;
    write!(writer, "\"baseline_key\":\"{}\"", baseline_key_str(&run.config))?;
    write!(writer, ",\"faulted\":")?;
    write_metrics_json(writer, &run.faulted_metrics)?;
    write!(writer, "}}")?;
    Ok(())
}

/// Format an f64 for JSON: NaN → "null", finite → formatted number.
fn json_f64(v: f64, precision: usize) -> String {
    if v.is_nan() { "null".to_string() } else { format!("{:.prec$}", v, prec = precision) }
}

fn write_metrics_json<W: Write>(
    writer: &mut W,
    m: &super::metrics::RunMetrics,
) -> std::io::Result<()> {
    write!(
        writer,
        "{{\"avg_throughput\":{},\"total_tasks\":{},\"unassigned_ratio\":{},\
         \"wait_ratio\":{},\"fault_tolerance\":{},\
         \"critical_time\":{},\"deficit_recovery\":{},\"throughput_recovery\":{},\"mtbf\":{},\
         \"recovery_tick\":{},\
         \"survival_rate\":{},\"impacted_area\":{},\
         \"deficit_integral\":{},\
         \"cascade_depth_avg\":{},\"cascade_spread_avg\":{},\"fleet_utilization\":{},\
         \"solver_step_avg_us\":{},\"solver_step_max_us\":{},\
         \"wall_time_ms\":{}}}",
        json_f64(m.avg_throughput, 4),
        m.total_tasks,
        json_f64(m.unassigned_ratio, 4),
        json_f64(m.wait_ratio, 4),
        json_f64(m.fault_tolerance, 4),
        json_f64(m.critical_time, 4),
        json_f64(m.deficit_recovery, 2),
        json_f64(m.throughput_recovery, 2),
        m.mtbf.map_or("null".to_string(), |v| json_f64(v, 2)),
        m.recovery_tick.map_or("null".to_string(), |v| v.to_string()),
        json_f64(m.survival_rate, 4),
        json_f64(m.impacted_area, 4),
        m.deficit_integral,
        json_f64(m.cascade_depth_avg, 2),
        json_f64(m.cascade_spread_avg, 2),
        json_f64(m.fleet_utilization, 4),
        json_f64(m.solver_step_time_avg_us, 1),
        json_f64(m.solver_step_time_max_us, 1),
        m.wall_time_ms,
    )?;
    Ok(())
}

fn write_summary_json<W: Write>(writer: &mut W, s: &ConfigSummary) -> std::io::Result<()> {
    write!(
        writer,
        "{{\"solver\":\"{}\",\"topology\":\"{}\",\"scenario\":\"{}\",\
         \"scheduler\":\"{}\",\"num_agents\":{},\"num_seeds\":{},",
        s.solver_name,
        s.topology_name,
        s.scenario_label,
        s.scheduler_name,
        s.num_agents,
        s.num_seeds,
    )?;
    write_stat_json(writer, "throughput", &s.throughput)?;
    write!(writer, ",")?;
    write_stat_json(writer, "total_tasks", &s.total_tasks)?;
    write!(writer, ",")?;
    write_stat_json(writer, "unassigned_ratio", &s.unassigned_ratio)?;
    write!(writer, ",")?;
    write_stat_json(writer, "fault_tolerance", &s.fault_tolerance)?;
    write!(writer, ",")?;
    write_stat_json(writer, "critical_time", &s.critical_time)?;
    write!(writer, ",")?;
    write_stat_json(writer, "deficit_recovery", &s.deficit_recovery)?;
    write!(writer, ",")?;
    write_stat_json(writer, "throughput_recovery", &s.throughput_recovery)?;
    write!(writer, ",")?;
    write_stat_json(writer, "survival_rate", &s.survival_rate)?;
    write!(writer, ",")?;
    write_stat_json(writer, "impacted_area", &s.impacted_area)?;
    write!(writer, ",")?;
    write_stat_json(writer, "deficit_integral", &s.deficit_integral)?;
    write!(writer, ",")?;
    write_stat_json(writer, "cascade_depth", &s.cascade_depth)?;
    write!(writer, ",")?;
    write_stat_json(writer, "cascade_spread", &s.cascade_spread)?;
    write!(writer, ",")?;
    write_stat_json(writer, "fleet_utilization", &s.fleet_utilization)?;
    write!(writer, ",")?;
    write_stat_json(writer, "solver_step_us", &s.solver_step_us)?;
    write!(writer, ",")?;
    write_stat_json(writer, "wall_time_ms", &s.wall_time_ms)?;
    write!(writer, "}}")?;
    Ok(())
}

fn write_stat_json<W: Write>(
    writer: &mut W,
    name: &str,
    s: &super::stats::StatSummary,
) -> std::io::Result<()> {
    write!(
        writer,
        "\"{}\":{{\"n\":{},\"mean\":{},\"std\":{},\
         \"ci95_lo\":{},\"ci95_hi\":{},\"min\":{},\"max\":{}}}",
        name,
        s.n,
        json_f64(s.mean, 4),
        json_f64(s.std, 4),
        json_f64(s.ci95_lo, 4),
        json_f64(s.ci95_hi, 4),
        json_f64(s.min, 4),
        json_f64(s.max, 4),
    )
}

// ---------------------------------------------------------------------------
// JSON import
// ---------------------------------------------------------------------------

/// Parse a JSON file (produced by `write_matrix_json`) back into summary structs.
/// Returns only the summaries — runs are not reconstructed (too large for UI import).
pub fn parse_summaries_from_json(json: &str) -> Result<Vec<ConfigSummary>, String> {
    // Lightweight manual JSON parsing using serde_json::Value.
    let val: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("JSON parse error: {e}"))?;

    let summaries =
        val.get("summaries").and_then(|v| v.as_array()).ok_or("missing 'summaries' array")?;

    let mut result = Vec::with_capacity(summaries.len());
    for s in summaries {
        let solver = s.get("solver").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let topology = s.get("topology").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let scenario = s.get("scenario").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let scheduler = s.get("scheduler").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let num_agents = s.get("num_agents").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let num_seeds = s.get("num_seeds").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        result.push(ConfigSummary {
            solver_name: solver,
            topology_name: topology,
            scenario_label: scenario,
            scheduler_name: scheduler,
            num_agents,
            num_seeds,
            throughput: parse_stat(s, "throughput"),
            total_tasks: parse_stat(s, "total_tasks"),
            unassigned_ratio: parse_stat(s, "unassigned_ratio"),
            fault_tolerance: parse_stat(s, "fault_tolerance"),
            critical_time: parse_stat(s, "critical_time"),
            deficit_recovery: parse_stat(s, "deficit_recovery"),
            throughput_recovery: parse_stat(s, "throughput_recovery"),
            survival_rate: parse_stat(s, "survival_rate"),
            impacted_area: parse_stat(s, "impacted_area"),
            deficit_integral: parse_stat(s, "deficit_integral"),
            cascade_depth: parse_stat(s, "cascade_depth"),
            cascade_spread: parse_stat(s, "cascade_spread"),
            fleet_utilization: parse_stat(s, "fleet_utilization"),
            solver_step_us: parse_stat(s, "solver_step_us"),
            wall_time_ms: parse_stat(s, "wall_time_ms"),
        });
    }

    Ok(result)
}

fn parse_stat(parent: &serde_json::Value, key: &str) -> super::stats::StatSummary {
    match parent.get(key) {
        Some(v) => super::stats::StatSummary {
            n: v.get("n").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            mean: v.get("mean").and_then(|x| x.as_f64()).unwrap_or(0.0),
            std: v.get("std").and_then(|x| x.as_f64()).unwrap_or(0.0),
            ci95_lo: v.get("ci95_lo").and_then(|x| x.as_f64()).unwrap_or(0.0),
            ci95_hi: v.get("ci95_hi").and_then(|x| x.as_f64()).unwrap_or(0.0),
            min: v.get("min").and_then(|x| x.as_f64()).unwrap_or(0.0),
            max: v.get("max").and_then(|x| x.as_f64()).unwrap_or(0.0),
        },
        None => super::stats::StatSummary::default(),
    }
}

// ---------------------------------------------------------------------------
// LaTeX table export
// ---------------------------------------------------------------------------

/// Metric column identifiers for table/chart export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricColumn {
    Throughput,
    TotalTasks,
    UnassignedRatio,
    FaultTolerance,
    CriticalTime,
    DeficitRecovery,
    ThroughputRecovery,
    SurvivalRate,
    ImpactedArea,
    DeficitIntegral,
    CascadeDepth,
    CascadeSpread,
    FleetUtilization,
    SolverStepUs,
    WallTimeMs,
}

impl MetricColumn {
    pub fn label(self) -> &'static str {
        match self {
            Self::Throughput => "Throughput",
            Self::TotalTasks => "Tasks",
            Self::UnassignedRatio => "Unassigned %",
            Self::FaultTolerance => "FT",
            Self::CriticalTime => "Crit. Time",
            Self::DeficitRecovery => "Deficit Rec.",
            Self::ThroughputRecovery => "TP Rec.",
            Self::SurvivalRate => "Survival",
            Self::ImpactedArea => "Impact Area",
            Self::DeficitIntegral => "Deficit",
            Self::CascadeDepth => "Casc. Depth",
            Self::CascadeSpread => "Casc. Spread",
            Self::FleetUtilization => "Fleet Util.",
            Self::SolverStepUs => "Solver µs",
            Self::WallTimeMs => "Wall ms",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Throughput => "TP",
            Self::TotalTasks => "Tasks",
            Self::UnassignedRatio => "Unassigned",
            Self::FaultTolerance => "FT",
            Self::CriticalTime => "CT",
            Self::DeficitRecovery => "DefRec",
            Self::ThroughputRecovery => "TPRec",
            Self::SurvivalRate => "Surv",
            Self::ImpactedArea => "Impact",
            Self::DeficitIntegral => "Deficit",
            Self::CascadeDepth => "CascD",
            Self::CascadeSpread => "CascS",
            Self::FleetUtilization => "FUtil",
            Self::SolverStepUs => "µs",
            Self::WallTimeMs => "ms",
        }
    }

    pub fn get_stat(self, s: &ConfigSummary) -> &super::stats::StatSummary {
        match self {
            Self::Throughput => &s.throughput,
            Self::TotalTasks => &s.total_tasks,
            Self::UnassignedRatio => &s.unassigned_ratio,
            Self::FaultTolerance => &s.fault_tolerance,
            Self::CriticalTime => &s.critical_time,
            Self::DeficitRecovery => &s.deficit_recovery,
            Self::ThroughputRecovery => &s.throughput_recovery,
            Self::SurvivalRate => &s.survival_rate,
            Self::ImpactedArea => &s.impacted_area,
            Self::DeficitIntegral => &s.deficit_integral,
            Self::CascadeDepth => &s.cascade_depth,
            Self::CascadeSpread => &s.cascade_spread,
            Self::FleetUtilization => &s.fleet_utilization,
            Self::SolverStepUs => &s.solver_step_us,
            Self::WallTimeMs => &s.wall_time_ms,
        }
    }

    /// Format precision for display.
    pub fn decimals(self) -> usize {
        match self {
            Self::TotalTasks | Self::DeficitIntegral | Self::WallTimeMs => 0,
            Self::SolverStepUs | Self::DeficitRecovery | Self::ThroughputRecovery => 1,
            Self::Throughput
            | Self::FaultTolerance
            | Self::CriticalTime
            | Self::SurvivalRate
            | Self::UnassignedRatio
            | Self::ImpactedArea
            | Self::CascadeDepth
            | Self::CascadeSpread
            | Self::FleetUtilization => 2,
        }
    }

    pub const ALL: &'static [MetricColumn] = &[
        Self::Throughput,
        Self::TotalTasks,
        Self::UnassignedRatio,
        Self::FaultTolerance,
        Self::CriticalTime,
        Self::DeficitRecovery,
        Self::ThroughputRecovery,
        Self::SurvivalRate,
        Self::ImpactedArea,
        Self::DeficitIntegral,
        Self::CascadeDepth,
        Self::CascadeSpread,
        Self::FleetUtilization,
        Self::SolverStepUs,
        Self::WallTimeMs,
    ];
}

/// Write a booktabs-style LaTeX table.
pub fn write_latex_table<W: Write>(
    writer: &mut W,
    summaries: &[ConfigSummary],
    columns: &[MetricColumn],
) -> std::io::Result<()> {
    // Column spec: l for config cols + r for each metric
    let metric_cols = "r".repeat(columns.len());
    writeln!(writer, "\\begin{{tabular}}{{llllr{metric_cols}}}")?;
    writeln!(writer, "\\toprule")?;

    // Header
    write!(writer, "Solver & Topology & Scenario & Scheduler & Agents")?;
    for col in columns {
        write!(writer, " & {}", col.label())?;
    }
    writeln!(writer, " \\\\")?;
    writeln!(writer, "\\midrule")?;

    // Data rows
    for s in summaries {
        write!(
            writer,
            "{} & {} & {} & {} & {}",
            s.solver_name, s.topology_name, s.scenario_label, s.scheduler_name, s.num_agents,
        )?;
        for col in columns {
            let stat = col.get_stat(s);
            let d = col.decimals();
            write!(writer, " & ${:.prec$} \\pm {:.prec$}$", stat.mean, stat.std, prec = d)?;
        }
        writeln!(writer, " \\\\")?;
    }

    writeln!(writer, "\\bottomrule")?;
    writeln!(writer, "\\end{{tabular}}")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Typst table export
// ---------------------------------------------------------------------------

/// Write a Typst-formatted table.
pub fn write_typst_table<W: Write>(
    writer: &mut W,
    summaries: &[ConfigSummary],
    columns: &[MetricColumn],
) -> std::io::Result<()> {
    let total_cols = 5 + columns.len();
    writeln!(writer, "#table(")?;
    writeln!(writer, "  columns: {total_cols},")?;

    // Header
    write!(writer, "  [*Solver*], [*Topology*], [*Scenario*], [*Scheduler*], [*Agents*]")?;
    for col in columns {
        write!(writer, ", [*{}*]", col.label())?;
    }
    writeln!(writer, ",")?;

    // Data rows
    for s in summaries {
        write!(
            writer,
            "  [{}], [{}], [{}], [{}], [{}]",
            s.solver_name, s.topology_name, s.scenario_label, s.scheduler_name, s.num_agents,
        )?;
        for col in columns {
            let stat = col.get_stat(s);
            let d = col.decimals();
            write!(writer, ", [{:.prec$} ± {:.prec$}]", stat.mean, stat.std, prec = d)?;
        }
        writeln!(writer, ",")?;
    }

    writeln!(writer, ")")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SVG bar chart
// ---------------------------------------------------------------------------

/// Write a simple horizontal SVG bar chart for a single metric.
pub fn write_svg_chart<W: Write>(
    writer: &mut W,
    summaries: &[ConfigSummary],
    metric: MetricColumn,
    sorted_indices: &[usize],
) -> std::io::Result<()> {
    let bar_h = 28.0_f64;
    let gap = 4.0_f64;
    let label_w = 220.0_f64;
    let chart_w = 400.0_f64;
    let total_w = label_w + chart_w + 80.0; // extra for value text
    let total_h = (bar_h + gap) * sorted_indices.len() as f64 + 40.0;

    // Find max value for scaling
    let max_val = sorted_indices
        .iter()
        .map(|&i| metric.get_stat(&summaries[i]).mean)
        .fold(0.0_f64, f64::max)
        .max(1e-9);

    writeln!(
        writer,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {total_w} {total_h}\" \
         font-family=\"monospace\" font-size=\"12\">"
    )?;

    // Title
    writeln!(
        writer,
        "<text x=\"{label_w}\" y=\"16\" font-size=\"14\" font-weight=\"bold\">{}</text>",
        metric.label()
    )?;

    let y_start = 30.0;
    for (row, &idx) in sorted_indices.iter().enumerate() {
        let s = &summaries[idx];
        let stat = metric.get_stat(s);
        let y = y_start + row as f64 * (bar_h + gap);
        let w = (stat.mean / max_val) * chart_w;

        // Label
        let label = format!("{}/{}/{}", s.solver_name, s.scenario_label, s.num_agents);
        writeln!(
            writer,
            "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" dominant-baseline=\"middle\">{label}</text>",
            label_w - 6.0,
            y + bar_h / 2.0,
        )?;

        // Bar color based on zone
        let color = zone_color_hex(metric, stat.mean);
        writeln!(
            writer,
            "<rect x=\"{label_w}\" y=\"{y}\" width=\"{w:.1}\" height=\"{bar_h}\" fill=\"{color}\" rx=\"2\"/>",
        )?;

        // CI whisker
        let ci_lo_x = label_w + (stat.ci95_lo / max_val) * chart_w;
        let ci_hi_x = label_w + (stat.ci95_hi / max_val) * chart_w;
        let mid_y = y + bar_h / 2.0;
        writeln!(
            writer,
            "<line x1=\"{ci_lo_x:.1}\" y1=\"{mid_y}\" x2=\"{ci_hi_x:.1}\" y2=\"{mid_y}\" \
             stroke=\"#333\" stroke-width=\"1.5\"/>",
        )?;

        // Value text
        let d = metric.decimals();
        writeln!(
            writer,
            "<text x=\"{}\" y=\"{}\" dominant-baseline=\"middle\">{:.prec$}</text>",
            label_w + chart_w + 6.0,
            y + bar_h / 2.0,
            stat.mean,
            prec = d,
        )?;
    }

    writeln!(writer, "</svg>")?;
    Ok(())
}

/// Return a hex color based on metric zone thresholds.
fn zone_color_hex(col: MetricColumn, val: f64) -> &'static str {
    match col {
        MetricColumn::FaultTolerance | MetricColumn::SurvivalRate => {
            if val >= 0.7 {
                "#78b478"
            } else if val >= 0.4 {
                "#c8aa64"
            } else {
                "#b45050"
            }
        }
        MetricColumn::CriticalTime | MetricColumn::ImpactedArea => {
            if val <= 0.2 {
                "#78b478"
            } else if val <= 0.5 {
                "#c8aa64"
            } else {
                "#b45050"
            }
        }
        MetricColumn::DeficitRecovery | MetricColumn::ThroughputRecovery => {
            if val <= 20.0 {
                "#78b478"
            } else if val <= 60.0 {
                "#c8aa64"
            } else {
                "#b45050"
            }
        }
        _ => "#6688aa", // neutral for throughput, tasks, etc.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::config::ExperimentConfig;

    fn dummy_run() -> RunResult {
        crate::experiment::runner::run_single_experiment(&ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 3,
            seed: 42,
            tick_count: 20,
            custom_map: None,
        })
    }

    #[test]
    fn csv_runs_parses() {
        let run = dummy_run();
        let mut buf = Vec::new();
        write_runs_csv(&mut buf, &[run]).unwrap();
        let csv = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3); // header + baseline + faulted
        assert!(lines[0].starts_with("solver,"));
        assert!(lines[1].contains("true")); // is_baseline
        assert!(lines[2].contains("false"));
    }

    #[test]
    fn json_valid_structure() {
        let run = dummy_run();
        let result = MatrixResult {
            matrix: crate::experiment::config::ExperimentMatrix {
                solvers: vec!["pibt".into()],
                topologies: vec!["warehouse_large".into()],
                scenarios: vec![None],
                schedulers: vec!["random".into()],
                agent_counts: vec![3],
                seeds: vec![42],
                tick_count: 20,
            },
            runs: vec![run],
            summaries: vec![],
            wall_time_total_ms: 100,
        };
        let mut buf = Vec::new();
        write_matrix_json(&mut buf, &result).unwrap();
        let json = String::from_utf8(buf).unwrap();
        assert!(json.starts_with("{"));
        assert!(json.contains("\"baselines\":{"));
        assert!(json.contains("\"baseline_key\":"));
        assert!(json.contains("\"runs\":["));
        assert!(json.contains("\"summaries\":[]"));
    }

    #[test]
    fn json_roundtrip_summaries() {
        let matrix = crate::experiment::config::ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![3],
            seeds: vec![42, 123],
            tick_count: 20,
        };
        let result = crate::experiment::runner::run_matrix(&matrix, None);

        let mut buf = Vec::new();
        write_matrix_json(&mut buf, &result).unwrap();
        let json = String::from_utf8(buf).unwrap();

        let parsed = parse_summaries_from_json(&json).unwrap();
        assert_eq!(parsed.len(), result.summaries.len());
        assert_eq!(parsed[0].solver_name, "pibt");
        assert_eq!(parsed[0].num_seeds, 2);
        assert!((parsed[0].throughput.mean - result.summaries[0].throughput.mean).abs() < 0.001);
    }

    #[test]
    fn latex_table_output() {
        let matrix = crate::experiment::config::ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![3],
            seeds: vec![42],
            tick_count: 20,
        };
        let result = crate::experiment::runner::run_matrix(&matrix, None);

        let mut buf = Vec::new();
        write_latex_table(
            &mut buf,
            &result.summaries,
            &[MetricColumn::FaultTolerance, MetricColumn::Throughput],
        )
        .unwrap();
        let latex = String::from_utf8(buf).unwrap();
        assert!(latex.contains("\\toprule"));
        assert!(latex.contains("\\bottomrule"));
        assert!(latex.contains("\\pm"));
    }

    #[test]
    fn typst_table_output() {
        let matrix = crate::experiment::config::ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![3],
            seeds: vec![42],
            tick_count: 20,
        };
        let result = crate::experiment::runner::run_matrix(&matrix, None);

        let mut buf = Vec::new();
        write_typst_table(&mut buf, &result.summaries, &[MetricColumn::FaultTolerance]).unwrap();
        let typst = String::from_utf8(buf).unwrap();
        assert!(typst.contains("#table("));
        assert!(typst.contains("[*FT*]"));
    }

    #[test]
    fn svg_chart_output() {
        let matrix = crate::experiment::config::ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![3],
            seeds: vec![42],
            tick_count: 20,
        };
        let result = crate::experiment::runner::run_matrix(&matrix, None);

        let indices: Vec<usize> = (0..result.summaries.len()).collect();
        let mut buf = Vec::new();
        write_svg_chart(&mut buf, &result.summaries, MetricColumn::FaultTolerance, &indices)
            .unwrap();
        let svg = String::from_utf8(buf).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("<rect"));
    }
}
