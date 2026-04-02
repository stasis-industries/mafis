use super::state::DesktopUiState;
use super::theme;
use crate::analysis::history::TickHistory;
use crate::core::state::{SimState, SimulationConfig, StepMode};
use crate::fault::scenario::FaultSchedule;
use bevy::prelude::*;
use bevy_egui::EguiContexts;

pub fn timeline_ui(
    mut contexts: EguiContexts,
    ui_state: Res<DesktopUiState>,
    mut config: ResMut<SimulationConfig>,
    sim_state: Res<State<SimState>>,
    mut next_state: ResMut<NextState<SimState>>,
    mut step_mode: ResMut<StepMode>,
    history: Res<TickHistory>,
    schedule: Res<FaultSchedule>,
) -> Result {
    if !ui_state.show_timeline {
        return Ok(());
    }

    let ctx = match contexts.ctx_mut() {
        Ok(ctx) => ctx,
        Err(_) => return Ok(()),
    };

    let state = **sim_state;

    egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
        ui.horizontal(|ui| {
            // ── Playback controls ─────────────────────────────
            let btn_color = theme::TEXT_PRIMARY;
            let btn_dim = theme::TEXT_MUTED;

            match state {
                SimState::Idle => {
                    if ui.button(egui::RichText::new("START").color(btn_color).strong()).clicked() {
                        next_state.set(SimState::Loading);
                    }
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("PAUSE").color(btn_dim)),
                    );
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("STEP").color(btn_dim)),
                    );
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("RESET").color(btn_dim)),
                    );
                }
                SimState::Loading => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("START").color(btn_dim)),
                    );
                    ui.spinner();
                }
                SimState::Running => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("START").color(btn_dim)),
                    );
                    if ui.button(egui::RichText::new("PAUSE").color(btn_color)).clicked() {
                        next_state.set(SimState::Paused);
                    }
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("STEP").color(btn_dim)),
                    );
                    if ui.button(egui::RichText::new("RESET").color(btn_color)).clicked() {
                        next_state.set(SimState::Idle);
                    }
                }
                SimState::Paused | SimState::Replay => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("START").color(btn_dim)),
                    );
                    if ui.button(egui::RichText::new("RESUME").color(btn_color)).clicked() {
                        next_state.set(SimState::Running);
                    }
                    if ui.button(egui::RichText::new("STEP").color(btn_color)).clicked() {
                        step_mode.pending = true;
                        next_state.set(SimState::Running);
                    }
                    if ui.button(egui::RichText::new("RESET").color(btn_color)).clicked() {
                        next_state.set(SimState::Idle);
                    }
                }
                SimState::Finished => {
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("START").color(btn_dim)),
                    );
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("PAUSE").color(btn_dim)),
                    );
                    ui.add_enabled(
                        false,
                        egui::Button::new(egui::RichText::new("STEP").color(btn_dim)),
                    );
                    if ui.button(egui::RichText::new("RESET").color(btn_color)).clicked() {
                        next_state.set(SimState::Idle);
                    }
                }
            }

            ui.separator();

            // ── Progress bar + tick counter ────────────────────
            let tick = config.tick;
            let duration = config.duration;
            let progress = if duration > 0 { tick as f32 / duration as f32 } else { 0.0 };

            // Recorded range indicator
            let recorded = history.snapshots.len();
            if recorded > 0 {
                let max_recorded = history.snapshots.back().map(|s| s.tick).unwrap_or(0);
                ui.weak(format!("[rec: {}]", max_recorded));
            }

            let bar = egui::ProgressBar::new(progress).text(format!("{} / {}", tick, duration));
            ui.add(bar);

            // Replay cursor
            if state == SimState::Replay
                && let Some(cursor) = history.replay_cursor
            {
                ui.weak(format!("cursor: {cursor}"));
            }

            // ── Speed control (right-aligned) ─────────────────
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Hz");
                let mut hz = config.tick_hz as f32;
                let slider = egui::Slider::new(&mut hz, 1.0..=30.0).max_decimals(0);
                if ui.add(slider).changed() {
                    config.tick_hz = hz as f64;
                }
                ui.label("Speed");
            });
        });

        // Fault schedule markers (if any)
        if schedule.initialized && !schedule.events.is_empty() {
            ui.horizontal(|ui| {
                ui.weak("Faults:");
                for ev in &schedule.events {
                    let color = if ev.fired { theme::ZONE_POOR } else { theme::STATE_PAUSED };
                    let label = format!("t={}", ev.tick);
                    ui.colored_label(color, &label).on_hover_text(format!(
                        "{:?} {}",
                        ev.action,
                        if ev.fired { "(fired)" } else { "" }
                    ));
                }
            });
        }
    });

    Ok(())
}
