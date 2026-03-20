//! AnalysisEngine — per-tick metric recording for both headless and live paths.
//!
//! Reads `&SimulationRunner` + `&TickResult` after each tick, appends to time
//! series. Pure Rust — no Bevy dependency. Identical code path for baseline
//! and live simulation eliminates metric divergence by construction.

use bevy::prelude::IVec2;
use std::collections::HashMap;

use crate::analysis::TimeSeriesAccessor;
use crate::core::action::Action;
use crate::core::runner::{FaultRecord, SimulationRunner, TickResult};

// ---------------------------------------------------------------------------
// AnalysisEngine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AnalysisEngine {
    // Per-tick series (indexed by tick - 1)
    /// Instantaneous task completions per tick.
    pub throughput_series: Vec<f64>,
    /// Cumulative tasks completed.
    pub tasks_completed_series: Vec<u64>,
    /// Idle agent count per tick.
    pub idle_count_series: Vec<usize>,
    /// Agent positions per tick (opt-in — disabled for headless baseline to save memory).
    pub position_snapshots: Vec<Vec<IVec2>>,
    /// Whether to record position snapshots (false for headless experiment runs).
    pub record_positions: bool,
    /// Average heat across alive agents per tick.
    pub heat_series: Vec<f32>,
    /// Alive agent count per tick.
    pub alive_series: Vec<usize>,
    /// Dead agent count per tick.
    pub dead_series: Vec<usize>,
    /// Fault events per tick.
    pub fault_events: Vec<Vec<FaultRecord>>,
    /// Cumulative wait ratio (living_wait_actions / living_total_actions) at each tick.
    /// Measures congestion among alive agents — excludes dead agents.
    pub wait_ratio_series: Vec<f32>,

    // Running accumulators for wait_ratio (not serialized)
    cumulative_waits: u64,
    cumulative_actions: u64,

    // Cumulative / spatial — flat Vec indexed by (y * grid_w + x) for O(1) access
    traffic_counts_flat: Vec<u32>,
    traffic_grid_w: i32,
    traffic_grid_h: i32,

    // Aggregates (computed on demand via `compute_aggregates`)
    pub total_tasks: u64,
    pub avg_throughput: f64,
}

impl AnalysisEngine {
    /// Create a new engine, optionally pre-allocating for `capacity` ticks.
    pub fn new(capacity: usize) -> Self {
        Self {
            throughput_series: Vec::with_capacity(capacity),
            tasks_completed_series: Vec::with_capacity(capacity),
            idle_count_series: Vec::with_capacity(capacity),
            position_snapshots: Vec::new(),
            record_positions: false,
            heat_series: Vec::with_capacity(capacity),
            alive_series: Vec::with_capacity(capacity),
            dead_series: Vec::with_capacity(capacity),
            fault_events: Vec::with_capacity(capacity),
            wait_ratio_series: Vec::with_capacity(capacity),
            cumulative_waits: 0,
            cumulative_actions: 0,
            traffic_counts_flat: Vec::new(),
            traffic_grid_w: 0,
            traffic_grid_h: 0,
            total_tasks: 0,
            avg_throughput: 0.0,
        }
    }

    /// Record one tick of data from the runner and its result.
    ///
    /// Takes `result` by `&mut` so that `fault_events` can be moved out via
    /// `std::mem::take` instead of cloned -- avoids a per-tick Vec allocation.
    /// The caller's `fault_events` field is left as an empty Vec after this call.
    pub fn record_tick(&mut self, runner: &SimulationRunner, result: &mut TickResult) {
        self.throughput_series.push(result.throughput);
        self.tasks_completed_series.push(result.tasks_completed);
        self.idle_count_series.push(result.idle_count);
        self.heat_series.push(result.heat_avg);
        self.alive_series.push(result.alive_count);
        self.dead_series.push(result.dead_count);
        self.fault_events.push(std::mem::take(&mut result.fault_events));

        // Single pass: count alive agents and living waits simultaneously
        let mut alive_count = 0usize;
        let mut living_waits_this_tick = 0usize;
        for (i, a) in runner.agents.iter().enumerate() {
            if a.alive {
                alive_count += 1;
                if matches!(result.moves[i].action, Action::Wait) {
                    living_waits_this_tick += 1;
                }
            }
        }
        self.cumulative_waits += living_waits_this_tick as u64;
        self.cumulative_actions += alive_count as u64;
        let ratio = if self.cumulative_actions > 0 {
            self.cumulative_waits as f32 / self.cumulative_actions as f32
        } else {
            0.0
        };
        self.wait_ratio_series.push(ratio);

        // Position snapshot (opt-in — disabled for headless baseline)
        if self.record_positions {
            self.position_snapshots
                .push(runner.agents.iter().map(|a| a.pos).collect());
        }

        // Traffic accumulation — flat Vec for O(1) access, no hashing
        let grid = runner.grid();
        let gw = grid.width;
        let gh = grid.height;
        if gw != self.traffic_grid_w || gh != self.traffic_grid_h {
            let size = (gw * gh) as usize;
            self.traffic_counts_flat.clear();
            self.traffic_counts_flat.resize(size, 0);
            self.traffic_grid_w = gw;
            self.traffic_grid_h = gh;
        }
        for a in &runner.agents {
            let idx = (a.pos.y * gw + a.pos.x) as usize;
            if idx < self.traffic_counts_flat.len() {
                self.traffic_counts_flat[idx] += 1;
            }
        }
    }

    /// Export traffic counts as a HashMap (for serialization/bridge compatibility).
    pub fn traffic_counts(&self) -> HashMap<IVec2, u32> {
        let mut map = HashMap::new();
        let w = self.traffic_grid_w;
        for (idx, &count) in self.traffic_counts_flat.iter().enumerate() {
            if count > 0 {
                let x = (idx as i32) % w;
                let y = (idx as i32) / w;
                map.insert(IVec2::new(x, y), count);
            }
        }
        map
    }

    /// Compute aggregate metrics from accumulated series.
    pub fn compute_aggregates(&mut self) {
        self.total_tasks = self
            .tasks_completed_series
            .last()
            .copied()
            .unwrap_or(0);
        self.avg_throughput = if self.throughput_series.is_empty() {
            0.0
        } else {
            let sum: f64 = self.throughput_series.iter().sum();
            sum / self.throughput_series.len() as f64
        };
    }

    /// Number of recorded ticks.
    pub fn tick_count(&self) -> usize {
        self.throughput_series.len()
    }

    /// Truncate all series to keep only data up to `tick` (1-indexed).
    /// Used after rewind so stale future data doesn't persist.
    pub fn truncate_to_tick(&mut self, tick: u64) {
        let keep = tick as usize;
        self.throughput_series.truncate(keep);
        self.tasks_completed_series.truncate(keep);
        self.idle_count_series.truncate(keep);
        self.position_snapshots.truncate(keep);
        self.heat_series.truncate(keep);
        self.alive_series.truncate(keep);
        self.dead_series.truncate(keep);
        self.fault_events.truncate(keep);
        self.wait_ratio_series.truncate(keep);
        // Can't partially undo traffic — reset flat vec
        self.traffic_counts_flat.iter_mut().for_each(|c| *c = 0);
        // Recompute cumulative wait ratio accumulators from series
        self.cumulative_waits = 0;
        self.cumulative_actions = 0;
        // These will be rebuilt as new ticks are recorded
    }

    /// Clear all recorded data (for rewind/reset).
    pub fn clear(&mut self) {
        self.throughput_series.clear();
        self.tasks_completed_series.clear();
        self.idle_count_series.clear();
        self.position_snapshots.clear();
        self.heat_series.clear();
        self.alive_series.clear();
        self.dead_series.clear();
        self.fault_events.clear();
        self.wait_ratio_series.clear();
        self.cumulative_waits = 0;
        self.cumulative_actions = 0;
        self.traffic_counts_flat.iter_mut().for_each(|c| *c = 0);
        self.total_tasks = 0;
        self.avg_throughput = 0.0;
    }
}

// ---------------------------------------------------------------------------
// TimeSeriesAccessor impl
// ---------------------------------------------------------------------------

impl TimeSeriesAccessor for AnalysisEngine {
    fn throughput_series(&self) -> &[f64] { &self.throughput_series }
    fn tasks_completed_series(&self) -> &[u64] { &self.tasks_completed_series }
    fn idle_count_series(&self) -> &[usize] { &self.idle_count_series }
    fn wait_ratio_series(&self) -> &[f32] { &self.wait_ratio_series }
    fn position_snapshots(&self) -> &[Vec<IVec2>] { &self.position_snapshots }
}

// ---------------------------------------------------------------------------
// Conversion — AnalysisEngine → BaselineRecord (backward compat)
// ---------------------------------------------------------------------------

impl AnalysisEngine {
    /// Convert to a `BaselineRecord` for backward compatibility with the
    /// existing `BaselineStore`/`BaselineDiff` pipeline.
    pub fn into_baseline_record(
        self,
        config_hash: u64,
        tick_count: u64,
        num_agents: usize,
    ) -> super::baseline::BaselineRecord {
        // Convert flat traffic counts to HashMap for BaselineRecord
        let traffic_counts = self.traffic_counts();
        super::baseline::BaselineRecord {
            config_hash,
            tick_count,
            num_agents,
            throughput_series: self.throughput_series,
            tasks_completed_series: self.tasks_completed_series,
            idle_count_series: self.idle_count_series,
            wait_ratio_series: self.wait_ratio_series,
            total_tasks: self.total_tasks,
            avg_throughput: self.avg_throughput,
            traffic_counts,
            position_snapshots: self.position_snapshots,
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
    fn new_engine_is_empty() {
        let e = AnalysisEngine::new(100);
        assert_eq!(e.tick_count(), 0);
        assert_eq!(e.total_tasks, 0);
        assert_eq!(e.avg_throughput, 0.0);
        assert!(e.traffic_counts().is_empty());
    }

    #[test]
    fn compute_aggregates_on_empty() {
        let mut e = AnalysisEngine::new(0);
        e.compute_aggregates();
        assert_eq!(e.total_tasks, 0);
        assert_eq!(e.avg_throughput, 0.0);
    }

    #[test]
    fn throughput_at_clamped() {
        let mut e = AnalysisEngine::new(0);
        e.throughput_series = vec![1.0, 2.0, 3.0];
        // 1-based ticks: tick 1 = index 0, tick 2 = index 1, etc.
        assert_eq!(e.throughput_at(1), 1.0);
        assert_eq!(e.throughput_at(2), 2.0);
        assert_eq!(e.throughput_at(3), 3.0);
        // Tick 0 (before sim) — saturating_sub(1)=0, returns series[0]
        assert_eq!(e.throughput_at(0), 1.0);
        // Beyond range — clamped to last
        assert_eq!(e.throughput_at(999), 3.0);
    }

    #[test]
    fn tasks_at_clamped() {
        let mut e = AnalysisEngine::new(0);
        e.tasks_completed_series = vec![0, 1, 3];
        // 1-based ticks
        assert_eq!(e.tasks_at(1), 0);
        assert_eq!(e.tasks_at(3), 3);
        assert_eq!(e.tasks_at(100), 3);
    }

    #[test]
    fn idle_at_clamped() {
        let mut e = AnalysisEngine::new(0);
        e.idle_count_series = vec![5, 3, 1];
        // 1-based ticks
        assert_eq!(e.idle_at(1), 5);
        assert_eq!(e.idle_at(3), 1);
        assert_eq!(e.idle_at(50), 1);
    }

    #[test]
    fn positions_at_returns_none_for_tick_zero() {
        let e = AnalysisEngine::new(0);
        // tick 0 → index saturating_sub(1) = usize::MAX wraps, but get() returns None
        assert!(e.positions_at(0).is_none());
    }

    #[test]
    fn clear_resets_everything() {
        let mut e = AnalysisEngine::new(0);
        e.throughput_series.push(1.0);
        e.tasks_completed_series.push(5);
        e.idle_count_series.push(2);
        e.heat_series.push(0.5);
        e.alive_series.push(10);
        e.dead_series.push(0);
        e.traffic_counts_flat = vec![0, 42, 0, 0];
        e.traffic_grid_w = 2;
        e.traffic_grid_h = 2;
        e.total_tasks = 5;
        e.avg_throughput = 1.0;

        e.clear();

        assert_eq!(e.tick_count(), 0);
        assert_eq!(e.total_tasks, 0);
        assert_eq!(e.avg_throughput, 0.0);
        assert!(e.traffic_counts().is_empty());
    }

    #[test]
    fn compute_aggregates_correct() {
        let mut e = AnalysisEngine::new(0);
        e.throughput_series = vec![1.0, 2.0, 3.0];
        e.tasks_completed_series = vec![1, 3, 6];
        e.compute_aggregates();
        assert_eq!(e.total_tasks, 6);
        assert!((e.avg_throughput - 2.0).abs() < 1e-10);
    }

    #[test]
    fn into_baseline_record_preserves_data() {
        let mut e = AnalysisEngine::new(0);
        e.throughput_series = vec![1.0, 2.0];
        e.tasks_completed_series = vec![1, 3];
        e.idle_count_series = vec![5, 4];
        e.position_snapshots = vec![
            vec![IVec2::new(1, 1)],
            vec![IVec2::new(2, 2)],
        ];
        // Simulate traffic at (1,1) on a 3x3 grid
        e.traffic_grid_w = 3;
        e.traffic_grid_h = 3;
        e.traffic_counts_flat = vec![0; 9];
        e.traffic_counts_flat[1 * 3 + 1] = 1; // (1,1)
        e.total_tasks = 3;
        e.avg_throughput = 1.5;

        let record = e.into_baseline_record(42, 2, 1);
        assert_eq!(record.config_hash, 42);
        assert_eq!(record.tick_count, 2);
        assert_eq!(record.num_agents, 1);
        assert_eq!(record.total_tasks, 3);
        assert_eq!(record.throughput_series.len(), 2);
        assert_eq!(record.position_snapshots.len(), 2);
    }
}
