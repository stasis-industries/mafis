use std::sync::atomic::Ordering;

use crate::core::topology::TopologyRegistry;
use crate::experiment::config::ExperimentMatrix;
use crate::experiment::export::MetricColumn;
use crate::experiment::runner::MatrixResult;
use crate::solver::SOLVER_NAMES;

use super::{ExperimentCommand, ExperimentGuiState, ExperimentHandle, ExpStage, SortColumn};
use super::helpers::{
    PRESETS, TABLE_METRICS, export_button, metric_zone_color, sortable_header, sync_topologies,
    ExportFormat,
};

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
    #[cfg_attr(feature = "headless", allow(unused))] commands: &mut Vec<ExperimentCommand>,
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
                summaries.len(),
                num_runs,
                wall_ms as f64 / 1000.0,
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
        && sel_idx < summaries.len()
    {
        let sel = &summaries[sel_idx];
        ui.add_space(4.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!(
                "{} / {} / {} / {} / {}a",
                sel.solver_name,
                sel.topology_name,
                sel.scenario_label,
                sel.scheduler_name,
                sel.num_agents,
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
                ui.weak("No per-run data (imported from summaries only)");
            } else {
                egui::ScrollArea::vertical().max_height(120.0).id_salt("fp_drill_down").show(
                    ui,
                    |ui| {
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
                                        metric_zone_color(
                                            MetricColumn::FaultTolerance,
                                            f.fault_tolerance,
                                        ),
                                        format!("{:.2}", f.fault_tolerance),
                                    );
                                    ui.colored_label(
                                        metric_zone_color(
                                            MetricColumn::SurvivalRate,
                                            f.survival_rate,
                                        ),
                                        format!("{:.2}", f.survival_rate),
                                    );
                                    ui.monospace(format!("{:.1}", f.deficit_recovery));
                                    ui.monospace(format!("{}", f.total_tasks));
                                    ui.end_row();
                                }
                            });
                    },
                );
            }
        } else {
            // Summary detail for imported data
            egui::Grid::new("fp_drill_detail").striped(true).show(ui, |ui| {
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
                    ui.monospace(format!(
                        "[{:.prec$}, {:.prec$}]",
                        stat.ci95_lo,
                        stat.ci95_hi,
                        prec = d
                    ));
                    ui.end_row();
                }
            });
        }
    }
}
