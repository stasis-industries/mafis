use egui_plot::{Line, Plot, PlotPoints};

use crate::analysis::baseline::BaselineRecord;
use crate::analysis::engine::AnalysisEngine;
use super::{CHART_SECONDARY, CHART_BASELINE};

pub fn tasks_chart(
    ui: &mut egui::Ui,
    analysis: &AnalysisEngine,
    baseline: Option<&BaselineRecord>,
) {
    if analysis.tasks_completed_series.is_empty() {
        return;
    }

    let live: PlotPoints = analysis.tasks_completed_series.iter()
        .enumerate()
        .map(|(i, &v)| [(i + 1) as f64, v as f64])
        .collect();

    Plot::new("tasks_chart")
        .height(140.0)
        .x_axis_label("Tick")
        .y_axis_label("Tasks")
        .legend(egui_plot::Legend::default())
        .show(ui, |plot_ui| {
            plot_ui.line(
                Line::new("Live", live)
                    .color(CHART_SECONDARY)
                    .width(2.0),
            );

            if let Some(bl) = baseline {
                let bl_points: PlotPoints = bl.tasks_completed_series.iter()
                    .enumerate()
                    .map(|(i, &v)| [(i + 1) as f64, v as f64])
                    .collect();
                plot_ui.line(
                    Line::new("Baseline", bl_points)
                        .color(CHART_BASELINE)
                        .width(1.5)
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
            }
        });
}
