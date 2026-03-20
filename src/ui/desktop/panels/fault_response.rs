use crate::analysis::fault_metrics::FaultMetrics;
use super::super::theme;

pub fn fault_response_panel(ui: &mut egui::Ui, metrics: &FaultMetrics) {
    let has_events = !metrics.event_records.is_empty();

    egui::Grid::new("fault_resp_grid").show(ui, |ui| {
        // Fault count
        ui.label("Faults");
        ui.monospace(format!("{}", metrics.event_records.len()));
        ui.end_row();

        // MTTR
        ui.label("MTTR");
        if metrics.mttr > 0.0 {
            ui.monospace(format!("{:.1} ticks", metrics.mttr));
        } else {
            ui.weak("—");
        }
        ui.end_row();

        // MTBF
        ui.label("MTBF");
        match metrics.mtbf {
            Some(v) => ui.monospace(format!("{v:.1} ticks")),
            None => ui.weak("—"),
        };
        ui.end_row();

        // Propagation rate
        ui.label("Propagation");
        ui.monospace(format!("{:.1}%", metrics.propagation_rate * 100.0));
        ui.end_row();

        // Recovery rate
        ui.label("Recovery");
        let color = if metrics.recovery_rate >= 0.8 {
            theme::ZONE_GOOD
        } else if metrics.recovery_rate >= 0.5 {
            theme::ZONE_FAIR
        } else {
            theme::ZONE_POOR
        };
        ui.colored_label(color, format!("{:.0}%", metrics.recovery_rate * 100.0));
        ui.end_row();

        // Survival rate
        let alive = metrics.initial_agent_count.saturating_sub(metrics.total_affected - metrics.total_recovered);
        let survival = if metrics.initial_agent_count > 0 {
            alive as f32 / metrics.initial_agent_count as f32
        } else {
            1.0
        };
        ui.label("Survival");
        ui.monospace(format!("{:.0}%", survival * 100.0));
        ui.end_row();
    });

    // Recent fault events
    if has_events {
        ui.add_space(6.0);
        ui.label("Recent Events");
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .show(ui, |ui| {
                for ev in metrics.event_records.iter().rev().take(10) {
                    ui.horizontal(|ui| {
                        ui.weak(format!("t={}", ev.tick));
                        ui.label(format!("{:?}", ev.fault_type));
                        if ev.agents_affected > 0 {
                            ui.weak(format!("({})", ev.agents_affected));
                        }
                    });
                }
            });
    }
}
