use crate::analysis::TimeSeriesAccessor;
use crate::analysis::baseline::{BaselineDiff, BaselineStore};
use crate::core::live_sim::LiveSim;
use super::super::theme;

pub fn performance_panel(
    ui: &mut egui::Ui,
    live_sim: Option<&LiveSim>,
    baseline_store: &BaselineStore,
    baseline_diff: &BaselineDiff,
) {
    let Some(sim) = live_sim else {
        ui.weak("No simulation running");
        return;
    };

    let analysis = &sim.analysis;

    egui::Grid::new("perf_grid").show(ui, |ui| {
        // Throughput
        let tp = analysis.throughput_series.last().copied().unwrap_or(0.0);
        ui.label("Throughput");
        ui.monospace(format!("{tp:.1} goals/tick"));
        ui.end_row();

        // With baseline delta
        if let Some(ref baseline) = baseline_store.record {
            let bl_tp = baseline.avg_throughput;
            let delta = tp - bl_tp;
            let color = if delta >= 0.0 {
                theme::ZONE_GOOD
            } else {
                theme::ZONE_POOR
            };
            ui.label("");
            ui.colored_label(color, format!("{delta:+.2} vs baseline"));
            ui.end_row();
        }

        // Tasks completed
        ui.label("Tasks");
        ui.monospace(format!("{}", sim.runner.tasks_completed));
        ui.end_row();

        if let Some(ref baseline) = baseline_store.record {
            let bl_tasks = baseline.tasks_at(sim.runner.tick);
            let delta = sim.runner.tasks_completed as i64 - bl_tasks as i64;
            let color = if delta >= 0 {
                theme::ZONE_GOOD
            } else {
                theme::ZONE_POOR
            };
            ui.label("");
            ui.colored_label(color, format!("{delta:+} vs baseline"));
            ui.end_row();
        }

        // Idle ratio
        let idle = analysis.wait_ratio_series.last().copied().unwrap_or(0.0);
        ui.label("Idle Ratio");
        ui.monospace(format!("{:.1}%", idle * 100.0));
        ui.end_row();

        // Impacted area
        if baseline_diff.deficit_integral > 0 || baseline_diff.surplus_integral > 0 {
            ui.label("Impact Area");
            ui.monospace(format!("{:.1}%", baseline_diff.impacted_area));
            ui.end_row();
        }

        // Gap
        if baseline_diff.gap != 0 {
            let gap_color = if baseline_diff.gap > 0 {
                theme::ZONE_POOR
            } else {
                theme::ZONE_GOOD
            };
            ui.label("Gap");
            ui.colored_label(gap_color, format!("{}", baseline_diff.gap));
            ui.end_row();
        }

        // Recovery
        if let Some(tick) = baseline_diff.recovery_tick {
            ui.label("Recovery");
            ui.colored_label(theme::ZONE_GOOD, format!("tick {tick}"));
            ui.end_row();
        }
    });
}
