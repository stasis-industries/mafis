use std::sync::atomic::Ordering;

use crate::core::topology::TopologyRegistry;
use crate::experiment::config::ExperimentMatrix;
use crate::experiment::export::MetricColumn;
use crate::experiment::runner::MatrixResult;
use crate::solver::SOLVER_NAMES;

use super::{ExperimentCommand, ExperimentGuiState, ExperimentHandle, SortColumn};
use super::helpers::{
    PRESETS, TABLE_METRICS, export_button, metric_zone_color, sortable_header, sync_topologies,
    ExportFormat,
};

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
            let label = SOLVER_NAMES
                .iter()
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
                Ok(json) => match crate::experiment::export::parse_summaries_from_json(&json) {
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
                },
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
        ui.label(format!("{num_runs} runs in {:.1}s", wall_ms as f64 / 1000.0,));
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
                plot_ui.bar_chart(egui_plot::BarChart::new(chart_col.label(), bars));
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
        && sel_idx < summaries.len()
        && !runs.is_empty()
    {
        let sel = &summaries[sel_idx];
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.heading(format!(
                "Detail: {} / {} / {} / {} / {} agents",
                sel.solver_name,
                sel.topology_name,
                sel.scenario_label,
                sel.scheduler_name,
                sel.num_agents,
            ));
            if ui.small_button("Close").clicked() {
                gui.show_drill_down = false;
            }
        });

        // Filter runs matching this config
        let matching: Vec<&crate::experiment::runner::RunResult> = runs
            .iter()
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
            egui::ScrollArea::vertical().max_height(150.0).id_salt("drill_down_scroll").show(
                ui,
                |ui| {
                    egui::Grid::new("drill_down_table").striped(true).min_col_width(50.0).show(
                        ui,
                        |ui| {
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
                                    metric_zone_color(
                                        MetricColumn::FaultTolerance,
                                        f.fault_tolerance,
                                    ),
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
                        },
                    );
                },
            );
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
