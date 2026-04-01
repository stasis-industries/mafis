use bevy::prelude::*;

use crate::constants;
use crate::core::state::SimState;
use crate::solver::SOLVER_NAMES;
use crate::ui::controls::UiState;
use super::super::theme;

struct SolverInfo {
    optimality: &'static str,
    scalability: &'static str,
    description: &'static str,
    source: &'static str,
    warning: Option<&'static str>,
}

fn solver_info(id: &str) -> SolverInfo {
    match id {
        "pibt" => SolverInfo {
            optimality: "Suboptimal",
            scalability: "Excellent",
            description: "Reactive one-step priority inheritance. Replans every tick — fast, handles high density.",
            source: "Okumura et al., AAAI 2019",
            warning: None,
        },
        "rhcr_pbs" => SolverInfo {
            optimality: "Bounded",
            scalability: "Good",
            description: "Rolling-horizon with PBS windowed planner. Better path quality, higher compute cost.",
            source: "Li et al., AAAI 2021",
            warning: None,
        },
        "rhcr_pibt" => SolverInfo {
            optimality: "Suboptimal",
            scalability: "Good",
            description: "Rolling-horizon with unrolled PIBT windows. Cooperative multi-step planning.",
            source: "Li et al., AAAI 2021",
            warning: None,
        },
        "rhcr_priority_astar" => SolverInfo {
            optimality: "Optimal (per agent)",
            scalability: "Moderate",
            description: "Rolling-horizon with sequential spacetime A*. Good for moderate density.",
            source: "Li et al., AAAI 2021",
            warning: None,
        },
        "rt_lacam" => SolverInfo {
            optimality: "Suboptimal",
            scalability: "Excellent",
            description: "Real-time lazy constraint DFS with PIBT config generator, persistent search, and rerooting.",
            source: "Liang et al., SoCS 2025",
            warning: None,
        },
        "token_passing" => SolverInfo {
            optimality: "Optimal (per agent)",
            scalability: "Limited",
            description: "Decentralized sequential planning via shared TOKEN. Each agent plans against all others.",
            source: "Ma et al., AAMAS 2017",
            warning: Some("Recommended ≤100 agents"),
        },
        "tpts" => SolverInfo {
            optimality: "Optimal (per agent)",
            scalability: "Limited",
            description: "Token Passing with Task Swaps. A* cost swap evaluation with snapshot/restore.",
            source: "Ma et al., AAMAS 2017",
            warning: Some("Recommended ≤100 agents"),
        },
        _ => SolverInfo {
            optimality: "?",
            scalability: "?",
            description: "Unknown solver.",
            source: "",
            warning: None,
        },
    }
}

pub fn solver_panel(
    ui: &mut egui::Ui,
    ui_state: &mut UiState,
    sim_state: SimState,
) {
    let idle = sim_state == SimState::Idle;

    // ── Solver dropdown ────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Algorithm");
        let current_label = SOLVER_NAMES
            .iter()
            .find(|(id, _)| *id == ui_state.solver_name)
            .map(|(_, label)| *label)
            .unwrap_or("Unknown");

        egui::ComboBox::from_id_salt("solver_combo")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                for &(id, label) in SOLVER_NAMES {
                    let selected = ui_state.solver_name == id;
                    if ui.selectable_label(selected, label).clicked() && !selected && idle {
                        ui_state.solver_name = id.to_string();
                        // Clear RHCR overrides when switching solver
                        ui_state.rhcr_horizon = None;
                        ui_state.rhcr_replan_interval = None;
                        ui_state.rhcr_fallback = None;
                    }
                }
            });
    });

    // ── Solver info card ───────────────────────────────────────────
    let info = solver_info(&ui_state.solver_name);
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme::BG_CARD)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Opt:");
                ui.label(info.optimality);
                ui.separator();
                ui.strong("Scale:");
                ui.label(info.scalability);
            });
            ui.weak(info.description);
            if !info.source.is_empty() {
                ui.add_space(2.0);
                ui.weak(info.source);
            }
            if let Some(warn) = info.warning {
                ui.colored_label(theme::STATE_PAUSED, warn);
            }
        });

    // ── RHCR settings (only for RHCR solvers) ──────────────────────
    let is_rhcr = ui_state.solver_name.starts_with("rhcr");
    if is_rhcr {
        ui.add_space(4.0);
        ui.label("RHCR Settings");
        ui.indent("rhcr_indent", |ui| {
            // Horizon
            ui.horizontal(|ui| {
                ui.label("Horizon (H)");
                let mut h = ui_state.rhcr_horizon.unwrap_or(0) as u32;
                let auto = ui_state.rhcr_horizon.is_none();
                let slider = egui::Slider::new(
                    &mut h,
                    constants::RHCR_MIN_HORIZON as u32..=constants::RHCR_MAX_HORIZON as u32,
                )
                .suffix(if auto { " (auto)" } else { "" });
                if ui.add_enabled(idle, slider).changed() {
                    ui_state.rhcr_horizon = Some(h as usize);
                }
                if auto {
                    ui.weak("auto");
                }
            });

            // Replan interval
            ui.horizontal(|ui| {
                ui.label("Replan (W)");
                let mut w = ui_state.rhcr_replan_interval.unwrap_or(0) as u32;
                let auto = ui_state.rhcr_replan_interval.is_none();
                let slider = egui::Slider::new(
                    &mut w,
                    constants::RHCR_MIN_REPLAN_INTERVAL as u32
                        ..=constants::RHCR_MAX_REPLAN_INTERVAL as u32,
                );
                if ui.add_enabled(idle, slider).changed() {
                    ui_state.rhcr_replan_interval = Some(w as usize);
                }
                if auto {
                    ui.weak("auto");
                }
            });

            // Fallback mode
            ui.horizontal(|ui| {
                ui.label("Fallback");
                let current = ui_state
                    .rhcr_fallback
                    .as_deref()
                    .unwrap_or("auto")
                    .to_owned();
                egui::ComboBox::from_id_salt("rhcr_fallback")
                    .selected_text(&current)
                    .show_ui(ui, |ui| {
                        for mode in &["auto", "per_agent", "full", "tiered"] {
                            if ui
                                .selectable_label(current == *mode, *mode)
                                .clicked()
                                && idle
                            {
                                ui_state.rhcr_fallback = if *mode == "auto" {
                                    None
                                } else {
                                    Some(mode.to_string())
                                };
                            }
                        }
                    });
            });

            ui.weak("Auto-tuned for current grid and agent count.");
        });
    }
}
