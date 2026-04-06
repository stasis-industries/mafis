//! Tick History — full snapshot recording for rewind/replay.
//!
//! Records per-tick snapshots during the fault injection phase.
//! Supports forward/backward seek and fault-event navigation.

use bevy::prelude::*;
use serde::Serialize;
use std::collections::VecDeque;

use crate::constants;
use crate::core::agent::{AgentIndex, LogicalAgent};
use crate::core::live_sim::LiveSim;
use crate::core::phase::SimulationPhase;
use crate::core::seed::SeededRng;
use crate::core::state::SimulationConfig;
use crate::core::task::LifelongConfig;
use crate::fault::breakdown::Dead;
use crate::fault::heat::HeatState;

use super::cascade::CascadeState;
use super::fault_metrics::FaultMetrics;

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct AgentSnapshot {
    pub index: usize,
    pub pos: IVec2,
    pub goal: IVec2,
    pub heat: f32,
    pub is_dead: bool,
    pub plan_length: usize,
    pub task_leg: String,
    /// Extra positions needed to reconstruct TaskLeg variants.
    /// [pickup] for TravelToLoad/Loading, [from, to] for TravelToDeliver/Unloading.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub task_leg_data: Vec<[i32; 2]>,
    /// Full planned path encoded as bytes (Action::to_u8) for deterministic restore.
    /// Skipped from JSON — internal use only.
    #[serde(skip)]
    pub planned_actions: Vec<u8>,
    /// Weibull operational age (movement-ticks). Required for wear rollback.
    #[serde(skip)]
    pub operational_age: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MetricsSnapshot {
    pub wait_ratio: f32,
    pub fault_count: u32,
    pub cascade_max_depth: u32,
    pub cascade_total_cost: u32,
    pub survival_rate: f32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FullTickSnapshot {
    pub tick: u64,
    pub phase: String,
    pub agents: Vec<AgentSnapshot>,
    pub metrics: MetricsSnapshot,
    pub fault_event_count: u32,
    /// ChaCha8Rng stream word position at this tick — used to restore exact RNG state.
    #[serde(skip)]
    pub rng_word_pos: u128,
    /// Fault RNG stream word position — restored alongside rng_word_pos so fault
    /// detection replays identically after rewind.
    #[serde(skip)]
    pub fault_rng_word_pos: u128,
    /// Tasks completed at this tick — used to restore LifelongConfig after rewind.
    #[serde(skip)]
    pub lifelong_tasks_completed: u64,
    /// Solver priority state at this tick — used for deterministic rewind.
    #[serde(skip)]
    pub solver_priorities: Vec<f32>,
    /// Throughput completion_ticks window — restored on rewind so throughput
    /// doesn't drop to 0.0 after seek/rewind.
    #[serde(skip)]
    pub completion_ticks: std::collections::VecDeque<u64>,
    /// Per-agent intermittent `next_fault_tick` at this snapshot. Restored on
    /// rewind so intermittent faults replay deterministically and don't double-fire.
    /// Indexed by `AgentIndex` (matches runner.agents order).
    #[serde(skip)]
    pub intermittent_next_fault_tick: Vec<Option<u64>>,
}

// ---------------------------------------------------------------------------
// TickHistory resource
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Default)]
pub struct TickHistory {
    pub snapshots: VecDeque<FullTickSnapshot>,
    pub replay_cursor: Option<usize>,
    pub recording: bool,
    prev_fault_count: u32,
}

impl TickHistory {
    pub fn clear(&mut self) {
        self.snapshots.clear();
        self.replay_cursor = None;
        self.recording = false;
        self.prev_fault_count = 0;
    }

    /// Get the snapshot at the current replay cursor.
    pub fn current_snapshot(&self) -> Option<&FullTickSnapshot> {
        self.replay_cursor.and_then(|idx| self.snapshots.get(idx))
    }

    /// Convert a tick number to the nearest snapshot index.
    /// Returns the closest snapshot at or before the requested tick.
    pub fn tick_to_index(&self, tick: u64) -> Option<usize> {
        if self.snapshots.is_empty() {
            return None;
        }
        let idx = self.snapshots.partition_point(|s| s.tick <= tick);
        if idx == 0 {
            // All snapshots are after this tick — return first
            Some(0)
        } else {
            Some(idx - 1)
        }
    }

    /// Find the index of the previous fault event before `current_idx`.
    pub fn prev_fault_index(&self, current_idx: usize) -> Option<usize> {
        if current_idx == 0 {
            return None;
        }
        (0..current_idx).rev().find(|&i| self.snapshots[i].fault_event_count > 0)
    }

    /// Find the index of the next fault event after `current_idx`.
    pub fn next_fault_index(&self, current_idx: usize) -> Option<usize> {
        ((current_idx + 1)..self.snapshots.len()).find(|&i| self.snapshots[i].fault_event_count > 0)
    }

    /// Remove all snapshots after the given tick.
    /// Used when the simulation state is invalidated from a certain point
    /// (e.g., fault schedule modification or resume from rewind).
    pub fn truncate_after_tick(&mut self, tick: u64) {
        while let Some(last) = self.snapshots.back() {
            if last.tick > tick {
                self.snapshots.pop_back();
            } else {
                break;
            }
        }
        // Reset cursor if it's beyond the truncation point
        if let Some(cursor) = self.replay_cursor
            && cursor >= self.snapshots.len()
        {
            self.replay_cursor =
                if self.snapshots.is_empty() { None } else { Some(self.snapshots.len() - 1) };
        }
    }
}

impl AgentSnapshot {
    /// Reconstruct a TaskLeg from the snapshot's serialized label + data.
    pub fn reconstruct_task_leg(&self) -> crate::core::task::TaskLeg {
        use crate::core::task::TaskLeg;
        match self.task_leg.as_str() {
            "travel_empty" if !self.task_leg_data.is_empty() => {
                let p = self.task_leg_data[0];
                TaskLeg::TravelEmpty(IVec2::new(p[0], p[1]))
            }
            "loading" if !self.task_leg_data.is_empty() => {
                let p = self.task_leg_data[0];
                TaskLeg::Loading(IVec2::new(p[0], p[1]))
            }
            "travel_loaded" if self.task_leg_data.len() >= 2 => {
                let f = self.task_leg_data[0];
                let t = self.task_leg_data[1];
                TaskLeg::TravelLoaded { from: IVec2::new(f[0], f[1]), to: IVec2::new(t[0], t[1]) }
            }
            "unloading" if self.task_leg_data.len() >= 2 => {
                let f = self.task_leg_data[0];
                let t = self.task_leg_data[1];
                TaskLeg::Unloading { from: IVec2::new(f[0], f[1]), to: IVec2::new(t[0], t[1]) }
            }
            "travel_to_queue" if self.task_leg_data.len() >= 2 => {
                let f = self.task_leg_data[0];
                let t = self.task_leg_data[1];
                let li = self.task_leg_data.get(2).map(|d| d[0] as usize).unwrap_or(0);
                TaskLeg::TravelToQueue {
                    from: IVec2::new(f[0], f[1]),
                    to: IVec2::new(t[0], t[1]),
                    line_index: li,
                }
            }
            "queuing" if self.task_leg_data.len() >= 2 => {
                let f = self.task_leg_data[0];
                let t = self.task_leg_data[1];
                let li = self.task_leg_data.get(2).map(|d| d[0] as usize).unwrap_or(0);
                TaskLeg::Queuing {
                    from: IVec2::new(f[0], f[1]),
                    to: IVec2::new(t[0], t[1]),
                    line_index: li,
                }
            }
            "charging" => TaskLeg::Charging,
            _ => TaskLeg::Free,
        }
    }
}

// ---------------------------------------------------------------------------
// Recording system
// ---------------------------------------------------------------------------

pub fn record_tick_snapshot(
    sim_config: Res<SimulationConfig>,
    phase: Res<SimulationPhase>,
    agents: Query<(&LogicalAgent, &AgentIndex, Option<&HeatState>, Has<Dead>)>,
    sim: Option<Res<LiveSim>>,
    cascade: Res<CascadeState>,
    fault_metrics: Res<FaultMetrics>,
    rng: Res<SeededRng>,
    lifelong: Res<LifelongConfig>,
    solver: Res<crate::solver::ActiveSolver>,
    mut history: ResMut<TickHistory>,
) {
    // Skip ticks based on snapshot interval to reduce allocation pressure.
    // Always record the final tick so replay shows the true end state.
    let is_final_tick = sim_config.tick + 1 >= sim_config.duration;
    if crate::constants::TICK_SNAPSHOT_INTERVAL > 1
        && !sim_config.tick.is_multiple_of(crate::constants::TICK_SNAPSHOT_INTERVAL)
        && !is_final_tick
    {
        return;
    }

    history.recording = true;

    // If we resumed from an earlier tick, the buffer still has future snapshots.
    // Don't overwrite or truncate — only record when we pass the last snapshot.
    if let Some(last) = history.snapshots.back()
        && sim_config.tick <= last.tick
    {
        return;
    }

    let latest_survival =
        fault_metrics.survival_series.back().map(|(_, rate)| *rate).unwrap_or(1.0);

    let new_faults = cascade.fault_count.saturating_sub(history.prev_fault_count);
    history.prev_fault_count = cascade.fault_count;

    let mut agent_snapshots = Vec::with_capacity(agents.iter().len());
    for (agent, index, heat_state, is_dead) in &agents {
        use crate::core::task::TaskLeg;
        let (leg_label, leg_data) = match &agent.task_leg {
            TaskLeg::Free => ("free", vec![]),
            TaskLeg::TravelEmpty(p) => ("travel_empty", vec![[p.x, p.y]]),
            TaskLeg::Loading(p) => ("loading", vec![[p.x, p.y]]),
            TaskLeg::TravelLoaded { from, to } => {
                ("travel_loaded", vec![[from.x, from.y], [to.x, to.y]])
            }
            TaskLeg::Unloading { from, to } => ("unloading", vec![[from.x, from.y], [to.x, to.y]]),
            TaskLeg::Charging => ("charging", vec![]),
            TaskLeg::TravelToQueue { from, to, line_index } => {
                ("travel_to_queue", vec![[from.x, from.y], [to.x, to.y], [*line_index as i32, 0]])
            }
            TaskLeg::Queuing { from, to, line_index } => {
                ("queuing", vec![[from.x, from.y], [to.x, to.y], [*line_index as i32, 0]])
            }
        };
        // Read planned path from runner (zero-copy) when available,
        // fall back to ECS planned_path (rewind/legacy).
        let runner_path =
            sim.as_ref().and_then(|s| s.runner.agents.get(index.0)).map(|sa| &sa.planned_path);

        let (plan_len, actions): (usize, Vec<u8>) = if let Some(rp) = runner_path {
            (rp.len(), rp.iter().map(|&a| crate::core::action::Action::to_u8(a)).collect())
        } else {
            (
                agent.planned_path.len(),
                agent.planned_path.iter().map(|&a| crate::core::action::Action::to_u8(a)).collect(),
            )
        };

        let op_age = sim
            .as_ref()
            .and_then(|s| s.runner.agents.get(index.0))
            .map(|sa| sa.operational_age)
            .unwrap_or(0);

        agent_snapshots.push(AgentSnapshot {
            index: index.0,
            pos: agent.current_pos,
            goal: agent.goal_pos,
            heat: heat_state.map_or(0.0, |h| h.heat),
            is_dead,
            plan_length: plan_len,
            task_leg: leg_label.to_string(),
            task_leg_data: leg_data,
            planned_actions: actions,
            operational_age: op_age,
        });
    }

    let snapshot = FullTickSnapshot {
        tick: sim_config.tick,
        phase: phase.label().to_string(),
        agents: agent_snapshots,
        metrics: MetricsSnapshot {
            wait_ratio: fault_metrics.wait_ratio,
            fault_count: cascade.fault_count,
            cascade_max_depth: cascade.max_depth,
            cascade_total_cost: cascade.fault_count,
            survival_rate: latest_survival,
        },
        fault_event_count: new_faults,
        rng_word_pos: rng.rng.get_word_pos(),
        fault_rng_word_pos: sim
            .as_ref()
            .map(|s| s.runner.fault_rng().rng.get_word_pos())
            .unwrap_or(0),
        lifelong_tasks_completed: lifelong.tasks_completed,
        solver_priorities: solver.save_priorities(),
        completion_ticks: lifelong.completion_ticks().clone(),
        intermittent_next_fault_tick: sim
            .as_ref()
            .map(|s| s.runner.agents.iter().map(|sa| sa.next_fault_tick).collect())
            .unwrap_or_default(),
    };

    history.snapshots.push_back(snapshot);

    // Cap at MAX_TICK_HISTORY — O(1) pop_front with VecDeque
    while history.snapshots.len() > constants::MAX_TICK_HISTORY {
        history.snapshots.pop_front();
        if let Some(ref mut cursor) = history.replay_cursor {
            *cursor = cursor.saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_history_default_is_empty() {
        let h = TickHistory::default();
        assert!(h.snapshots.is_empty());
        assert!(h.replay_cursor.is_none());
        assert!(!h.recording);
    }

    #[test]
    fn tick_history_clear_resets_all() {
        let mut h = TickHistory::default();
        h.snapshots.push_back(FullTickSnapshot {
            tick: 1,
            phase: "fault_injection".into(),
            ..Default::default()
        });
        h.replay_cursor = Some(0);
        h.recording = true;

        h.clear();
        assert!(h.snapshots.is_empty());
        assert!(h.replay_cursor.is_none());
        assert!(!h.recording);
    }

    #[test]
    fn tick_to_index_finds_correct_tick() {
        let mut h = TickHistory::default();
        for t in [10, 11, 12, 15] {
            h.snapshots.push_back(FullTickSnapshot {
                tick: t,
                phase: "fault_injection".into(),
                ..Default::default()
            });
        }
        assert_eq!(h.tick_to_index(10), Some(0));
        assert_eq!(h.tick_to_index(12), Some(2));
        assert_eq!(h.tick_to_index(15), Some(3));
        // tick 99 is beyond all snapshots — returns last snapshot (nearest at-or-before)
        assert_eq!(h.tick_to_index(99), Some(3));
        // tick 5 is before all snapshots — returns first (index 0)
        assert_eq!(h.tick_to_index(5), Some(0));
    }

    #[test]
    fn fault_navigation_finds_events() {
        let mut h = TickHistory::default();
        let faults = [0, 1, 0, 0, 2, 0]; // faults at index 1 and 4
        for (i, fc) in faults.iter().enumerate() {
            h.snapshots.push_back(FullTickSnapshot {
                tick: i as u64,
                phase: "fault_injection".into(),
                fault_event_count: *fc,
                ..Default::default()
            });
        }

        // From index 3, prev fault is at 1
        assert_eq!(h.prev_fault_index(3), Some(1));
        // From index 0, no prev fault
        assert_eq!(h.prev_fault_index(0), None);
        // From index 2, next fault is at 4
        assert_eq!(h.next_fault_index(2), Some(4));
        // From index 5, no next fault
        assert_eq!(h.next_fault_index(5), None);
    }

    #[test]
    fn current_snapshot_with_cursor() {
        let mut h = TickHistory::default();
        h.snapshots.push_back(FullTickSnapshot {
            tick: 42,
            phase: "fault_injection".into(),
            ..Default::default()
        });
        assert!(h.current_snapshot().is_none());
        h.replay_cursor = Some(0);
        assert_eq!(h.current_snapshot().unwrap().tick, 42);
    }

    #[test]
    fn cap_respects_max_tick_history() {
        let mut h = TickHistory::default();
        // Push more than MAX
        for t in 0..(constants::MAX_TICK_HISTORY + 100) {
            h.snapshots.push_back(FullTickSnapshot {
                tick: t as u64,
                phase: "fault_injection".into(),
                ..Default::default()
            });
            // Simulate the cap logic from record_tick_snapshot
            while h.snapshots.len() > constants::MAX_TICK_HISTORY {
                h.snapshots.pop_front();
            }
        }
        assert_eq!(h.snapshots.len(), constants::MAX_TICK_HISTORY);
        // First tick should be 100
        assert_eq!(h.snapshots[0].tick, 100);
    }
}
