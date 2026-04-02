use egui_plot::{Line, Plot, PlotPoints};

use super::{CHART_BASELINE, CHART_PRIMARY, CHART_SECONDARY};
use crate::analysis::baseline::BaselineRecord;
use crate::analysis::engine::AnalysisEngine;
use crate::constants::THROUGHPUT_MA_WINDOW;

pub fn throughput_chart(
    ui: &mut egui::Ui,
    analysis: &AnalysisEngine,
    baseline: Option<&BaselineRecord>,
) {
    if analysis.throughput_series.is_empty() {
        return;
    }

    let raw: PlotPoints =
        analysis.throughput_series.iter().enumerate().map(|(i, &v)| [(i + 1) as f64, v]).collect();

    let mva = compute_mva(&analysis.throughput_series, THROUGHPUT_MA_WINDOW);
    let mva_points: PlotPoints =
        mva.iter().enumerate().map(|(i, &v)| [(i + 1) as f64, v]).collect();

    Plot::new("throughput_chart")
        .height(140.0)
        .x_axis_label("Tick")
        .y_axis_label("Goals/Tick")
        .legend(egui_plot::Legend::default())
        .show(ui, |plot_ui| {
            plot_ui.line(Line::new("Per-Tick", raw).color(CHART_PRIMARY).width(1.0));
            plot_ui.line(Line::new("MVA", mva_points).color(CHART_SECONDARY).width(2.0));

            if let Some(bl) = baseline {
                let bl_mva = compute_mva(&bl.throughput_series, THROUGHPUT_MA_WINDOW);
                let bl_mva_points: PlotPoints =
                    bl_mva.iter().enumerate().map(|(i, &v)| [(i + 1) as f64, v]).collect();
                plot_ui.line(
                    Line::new("Baseline", bl_mva_points)
                        .color(CHART_BASELINE)
                        .width(1.5)
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
            }
        });
}

fn compute_mva(data: &[f64], window: usize) -> Vec<f64> {
    if data.is_empty() || window == 0 {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(data.len());
    let mut sum = 0.0;
    for (i, &v) in data.iter().enumerate() {
        sum += v;
        if i >= window {
            sum -= data[i - window];
        }
        let count = (i + 1).min(window);
        result.push(sum / count as f64);
    }
    result
}
