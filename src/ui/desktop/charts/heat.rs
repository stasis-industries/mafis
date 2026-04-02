use egui_plot::{Line, Plot, PlotPoints};

use super::CHART_HEAT;
use crate::analysis::engine::AnalysisEngine;

pub fn heat_chart(ui: &mut egui::Ui, analysis: &AnalysisEngine) {
    if analysis.heat_series.is_empty() {
        return;
    }

    let points: PlotPoints =
        analysis.heat_series.iter().enumerate().map(|(i, &v)| [(i + 1) as f64, v as f64]).collect();

    Plot::new("heat_chart")
        .height(120.0)
        .x_axis_label("Tick")
        .y_axis_label("Avg Heat")
        .legend(egui_plot::Legend::default())
        .show(ui, |plot_ui| {
            plot_ui.line(Line::new("Avg Heat", points).color(CHART_HEAT).width(2.0));
        });
}
