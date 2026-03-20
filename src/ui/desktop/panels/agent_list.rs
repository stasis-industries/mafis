use egui::Color32;

use crate::constants::AGGREGATE_THRESHOLD;
use crate::core::live_sim::LiveSim;
use crate::core::runner::SimAgent;
use crate::fault::manual::ManualFaultCommand;
use super::super::theme;

pub struct AgentListOutput {
    pub manual_cmds: Vec<ManualFaultCommand>,
}

pub fn agent_list_panel(ui: &mut egui::Ui, live_sim: Option<&LiveSim>) -> AgentListOutput {
    let mut output = AgentListOutput { manual_cmds: Vec::new() };

    let Some(sim) = live_sim else {
        ui.weak("No simulation running");
        return output;
    };

    let agents = &sim.runner.agents;
    let n = agents.len();

    if n == 0 {
        ui.weak("No agents");
        return output;
    }

    if n > AGGREGATE_THRESHOLD {
        aggregate_view(ui, agents);
    } else {
        per_agent_view(ui, agents, &mut output);
    }

    output
}

fn per_agent_view(ui: &mut egui::Ui, agents: &[SimAgent], output: &mut AgentListOutput) {
    egui::ScrollArea::vertical()
        .max_height(250.0)
        .show(ui, |ui| {
            egui::Grid::new("agent_table")
                .striped(true)
                .min_col_width(24.0)
                .show(ui, |ui| {
                    // Header
                    ui.strong("ID");
                    ui.strong("Pos");
                    ui.strong("Goal");
                    ui.strong("Heat");
                    ui.strong("State");
                    ui.strong(""); // Actions column
                    ui.end_row();

                    for (i, a) in agents.iter().enumerate() {
                        ui.monospace(format!("{i}"));
                        ui.monospace(format!("{},{}", a.pos.x, a.pos.y));
                        ui.monospace(format!("{},{}", a.goal.x, a.goal.y));

                        // Heat with color
                        let heat_color = if a.heat > 60.0 {
                            theme::STATE_FAULT
                        } else if a.heat > 30.0 {
                            theme::STATE_PAUSED
                        } else {
                            theme::STATE_IDLE
                        };
                        ui.colored_label(heat_color, format!("{:.0}", a.heat));

                        // State
                        if !a.alive {
                            ui.colored_label(theme::STATE_FAULT, "DEAD");
                        } else if a.latency_remaining > 0 {
                            ui.colored_label(Color32::from_rgb(143, 58, 222), format!("LAT {}", a.latency_remaining));
                        } else {
                            ui.weak("OK");
                        }

                        // Action buttons (only for alive agents)
                        if a.alive {
                            ui.horizontal(|ui| {
                                let kill_btn = egui::Button::new(
                                    egui::RichText::new("K").color(theme::STATE_FAULT).small()
                                ).min_size(egui::vec2(20.0, 16.0));
                                if ui.add(kill_btn).on_hover_text("Kill this agent").clicked() {
                                    output.manual_cmds.push(ManualFaultCommand::KillAgent(i));
                                }

                                let slow_btn = egui::Button::new(
                                    egui::RichText::new("S").color(Color32::from_rgb(143, 58, 222)).small()
                                ).min_size(egui::vec2(20.0, 16.0));
                                if ui.add(slow_btn).on_hover_text("Inject latency (20 ticks)").clicked() {
                                    output.manual_cmds.push(ManualFaultCommand::InjectLatency { agent_id: i, duration: 20 });
                                }
                            });
                        } else {
                            ui.label("");
                        }

                        ui.end_row();
                    }
                });
        });
}

fn aggregate_view(ui: &mut egui::Ui, agents: &[SimAgent]) {
    let total = agents.len();
    let alive = agents.iter().filter(|a| a.alive).count();
    let dead = total - alive;
    let avg_heat: f32 = if alive > 0 {
        agents.iter().filter(|a| a.alive).map(|a| a.heat).sum::<f32>() / alive as f32
    } else {
        0.0
    };
    let max_heat = agents.iter().filter(|a| a.alive).map(|a| a.heat).fold(0.0f32, f32::max);

    egui::Grid::new("agent_summary").show(ui, |ui| {
        ui.label("Total");
        ui.monospace(format!("{total}"));
        ui.end_row();

        ui.label("Alive");
        ui.monospace(format!("{alive}"));
        ui.end_row();

        if dead > 0 {
            ui.label("Dead");
            ui.colored_label(theme::STATE_FAULT, format!("{dead}"));
            ui.end_row();
        }

        ui.label("Avg Heat");
        ui.monospace(format!("{avg_heat:.1}"));
        ui.end_row();

        ui.label("Max Heat");
        ui.monospace(format!("{max_heat:.1}"));
        ui.end_row();
    });

    // Heat distribution histogram
    ui.add_space(4.0);
    ui.label("Heat Distribution");
    let buckets = 8;
    let bucket_size = 100.0 / buckets as f32;
    let mut counts = vec![0usize; buckets];
    for a in agents.iter().filter(|a| a.alive) {
        let idx = ((a.heat / bucket_size) as usize).min(buckets - 1);
        counts[idx] += 1;
    }
    let max_count = counts.iter().copied().max().unwrap_or(1).max(1);

    ui.horizontal_wrapped(|ui| {
        for (i, &count) in counts.iter().enumerate() {
            let frac = count as f32 / max_count as f32;
            let bar = egui::ProgressBar::new(frac)
                .text(format!("{}-{}: {}", (i as f32 * bucket_size) as u32, ((i + 1) as f32 * bucket_size) as u32, count))
                .desired_width(ui.available_width());
            ui.add(bar);
        }
    });
}
