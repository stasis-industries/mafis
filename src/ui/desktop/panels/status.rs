use bevy::prelude::*;

use crate::core::live_sim::LiveSim;
use crate::core::state::{SimState, SimulationConfig};
use super::super::theme;

pub fn status_panel(
    ui: &mut egui::Ui,
    sim_state: &State<SimState>,
    config: &SimulationConfig,
    live_sim: Option<&LiveSim>,
) {
    let state = **sim_state;

    // State badge
    let (badge, color) = match state {
        SimState::Idle => ("IDLE", theme::STATE_IDLE),
        SimState::Loading => ("LOADING", theme::STATE_PAUSED),
        SimState::Running => ("RUNNING", theme::STATE_RUNNING),
        SimState::Paused => ("PAUSED", theme::STATE_PAUSED),
        SimState::Replay => ("REPLAY", egui::Color32::from_rgb(140, 140, 200)),
        SimState::Finished => ("FINISHED", theme::STATE_IDLE),
    };
    ui.colored_label(color, badge);

    // Tick / duration
    ui.separator();
    egui::Grid::new("status_grid").show(ui, |ui| {
        ui.label("Tick");
        ui.monospace(format!("{} / {}", config.tick, config.duration));
        ui.end_row();

        if let Some(sim) = live_sim {
            ui.label("Tasks");
            ui.monospace(format!("{}", sim.runner.tasks_completed));
            ui.end_row();
        }
    });

    if let Some(sim) = live_sim {
        let runner = &sim.runner;
        let total = runner.agents.len();
        let alive = runner.agents.iter().filter(|a| a.alive).count();
        let dead = total - alive;

        ui.add_space(4.0);

        // ── Task-leg bar (delivering / loading / idle) ─────────────
        let mut delivering = 0usize;
        let mut loading = 0usize;
        let mut idle_count = 0usize;
        for a in &runner.agents {
            if !a.alive { continue; }
            use crate::core::task::TaskLeg;
            match &a.task_leg {
                TaskLeg::TravelLoaded { .. }
                | TaskLeg::Unloading { .. }
                | TaskLeg::TravelToQueue { .. }
                | TaskLeg::Queuing { .. } => delivering += 1,
                TaskLeg::TravelEmpty(_)
                | TaskLeg::Loading(_) => loading += 1,
                TaskLeg::Free
                | TaskLeg::Charging => idle_count += 1,
            }
        }
        if alive > 0 {
            ui.label("Fleet Activity");
            let bar_width = ui.available_width();
            let (r, _) = ui.allocate_exact_size(egui::vec2(bar_width, 14.0), egui::Sense::hover());
            let painter = ui.painter_at(r);

            let del_frac = delivering as f32 / alive as f32;
            let load_frac = loading as f32 / alive as f32;

            let del_w = r.width() * del_frac;
            let load_w = r.width() * load_frac;

            // Delivering (green)
            painter.rect_filled(
                egui::Rect::from_min_size(r.min, egui::vec2(del_w, r.height())),
                0.0,
                theme::STATE_RUNNING,
            );
            // Loading (amber)
            painter.rect_filled(
                egui::Rect::from_min_size(r.min + egui::vec2(del_w, 0.0), egui::vec2(load_w, r.height())),
                0.0,
                theme::STATE_PAUSED,
            );
            // Idle (grey) — fills remaining
            painter.rect_filled(
                egui::Rect::from_min_size(
                    r.min + egui::vec2(del_w + load_w, 0.0),
                    egui::vec2(r.width() - del_w - load_w, r.height()),
                ),
                0.0,
                theme::TEXT_MUTED,
            );

            ui.horizontal(|ui| {
                ui.colored_label(theme::STATE_RUNNING, format!("Del {delivering}"));
                ui.colored_label(theme::STATE_PAUSED, format!("Load {loading}"));
                ui.weak(format!("Idle {idle_count}"));
            });
        }

        ui.add_space(4.0);

        // ── Fleet alive/dead bar ───────────────────────────────────
        ui.label(format!("Fleet  {total}"));
        let bar_width = ui.available_width();
        let (r, _) = ui.allocate_exact_size(egui::vec2(bar_width, 14.0), egui::Sense::hover());
        let painter = ui.painter_at(r);

        let alive_frac = if total > 0 { alive as f32 / total as f32 } else { 1.0 };
        let alive_w = r.width() * alive_frac;

        painter.rect_filled(
            egui::Rect::from_min_size(r.min, egui::vec2(alive_w, r.height())),
            0.0,
            theme::ZONE_GOOD,
        );
        if dead > 0 {
            painter.rect_filled(
                egui::Rect::from_min_size(r.min + egui::vec2(alive_w, 0.0), egui::vec2(r.width() - alive_w, r.height())),
                0.0,
                theme::STATE_FAULT,
            );
        }

        ui.horizontal(|ui| {
            ui.colored_label(theme::ZONE_GOOD, format!("Alive {alive}"));
            if dead > 0 {
                ui.colored_label(theme::STATE_FAULT, format!("Dead {dead}"));
            }
        });
    }
}
