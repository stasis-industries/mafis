use crate::export::config::{ExportConfig, ExportRequest, ExportTrigger};

pub struct ExportPanelOutput {
    pub export_request: Option<ExportRequest>,
}

pub fn export_panel(
    ui: &mut egui::Ui,
    export_config: &mut ExportConfig,
    is_running: bool,
) -> ExportPanelOutput {
    let mut output = ExportPanelOutput { export_request: None };

    // ── Auto-export triggers ───────────────────────────────────────
    ui.checkbox(&mut export_config.auto_on_finished, "Auto-export on finish");
    ui.checkbox(&mut export_config.auto_on_fault, "Auto-export on fault");

    // ── Periodic ───────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.checkbox(&mut export_config.periodic_enabled, "Periodic");
        let drag = egui::DragValue::new(&mut export_config.periodic_interval)
            .range(1..=1000)
            .suffix(" ticks");
        ui.add_enabled(export_config.periodic_enabled, drag);
    });

    ui.add_space(4.0);

    // ── Format selection ───────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.checkbox(&mut export_config.export_json, "JSON");
        ui.checkbox(&mut export_config.export_csv, "CSV");
    });

    ui.add_space(4.0);

    // ── Export Now button ──────────────────────────────────────────
    if ui.add_enabled(is_running, egui::Button::new("Export Now")).clicked() {
        output.export_request = Some(ExportRequest {
            trigger: ExportTrigger::Manual,
            json: export_config.export_json,
            csv: export_config.export_csv,
        });
    }

    output
}
