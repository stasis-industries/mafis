//! Fault Resilience Metrics Pipeline
//!
//! Computes research-grade KPIs for fault resilience analysis:
//! - MTTR (Mean Time To Recovery)
//! - Recovery rate
//! - Cascade spread
//! - Throughput (goals/tick)
//! - Idle ratio (aggregate)
//! - Fault survival rate (time-series)

use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::constants;
use crate::core::action::Action;
use crate::core::agent::{AgentActionStats, LastAction, LogicalAgent};
use crate::core::state::SimulationConfig;
use crate::fault::breakdown::Dead;
use crate::fault::config::{FaultSource, FaultType};

use super::cascade::CascadeState;

// ---------------------------------------------------------------------------
// FaultEventRecord — per-fault lifecycle tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FaultEventRecord {
    pub id: usize,
    pub tick: u64,
    pub fault_type: FaultType,
    pub source: FaultSource,
    pub position: IVec2,
    pub agents_affected: u32,
    pub cascade_depth: u32,
    pub throughput_before: f32,
    pub throughput_min: f32,
    pub throughput_delta: f32,
    pub recovered: bool,
    pub recovery_tick: Option<u64>,
}

// ---------------------------------------------------------------------------
// FaultMetrics resource
// ---------------------------------------------------------------------------

#[derive(Resource, Debug)]
pub struct FaultMetrics {
    recovery_pending: HashMap<Entity, u64>,
    recovery_times: Vec<u64>,
    pub mttr: f32,

    pub total_affected: u32,
    pub total_recovered: u32,
    pub recovery_rate: f32,

    cascade_spreads: Vec<u32>,
    pub avg_cascade_spread: f32,

    /// Mean Time Between Faults — average ticks between consecutive fault events.
    /// Requires 2+ events. (Or 2025)
    pub mtbf: Option<f32>,
    /// Propagation Rate — avg(agents_affected / alive_at_event) across all faults.
    /// Normalized 0-1 fraction of fleet affected per event.
    pub propagation_rate: f32,

    throughput_window: VecDeque<u32>,
    finished_entities: HashSet<Entity>,
    pub throughput: f32,

    pub idle_ratio: f32,

    pub survival_series: VecDeque<(u64, f32)>,
    pub initial_agent_count: u32,

    pub event_records: Vec<FaultEventRecord>,

    last_fault_log_len: usize,
}

impl Default for FaultMetrics {
    fn default() -> Self {
        Self {
            recovery_pending: HashMap::new(),
            recovery_times: Vec::new(),
            mttr: 0.0,
            total_affected: 0,
            total_recovered: 0,
            recovery_rate: 0.0,
            cascade_spreads: Vec::new(),
            avg_cascade_spread: 0.0,
            mtbf: None,
            propagation_rate: 0.0,
            throughput_window: VecDeque::with_capacity(constants::THROUGHPUT_WINDOW_SIZE + 1),
            finished_entities: HashSet::new(),
            throughput: 0.0,
            idle_ratio: 0.0,
            survival_series: VecDeque::with_capacity(constants::MAX_SURVIVAL_ENTRIES + 1),
            initial_agent_count: 0,
            event_records: Vec::new(),
            last_fault_log_len: 0,
        }
    }
}

impl FaultMetrics {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Truncate all data after `tick` for rewind support.
    pub fn truncate_after_tick(&mut self, tick: u64) {
        self.event_records.retain(|r| r.tick <= tick);
        self.survival_series.retain(|&(t, _)| t <= tick);
        self.throughput_window.clear();
        self.finished_entities.clear();
        // Will be recalculated by register_fault_recovery
        self.last_fault_log_len = 0;
    }

    /// Register a manual fault event directly (bypass cascade BFS).
    /// Used by `process_manual_faults` which runs in Update (Messages
    /// don't cross to FixedUpdate where propagate_cascade reads them).
    pub fn register_manual_event(
        &mut self,
        tick: u64,
        fault_type: FaultType,
        source: FaultSource,
        position: IVec2,
    ) {
        let id = self.event_records.len();
        self.event_records.push(FaultEventRecord {
            id,
            tick,
            fault_type,
            source,
            position,
            agents_affected: 1,
            cascade_depth: 0,
            throughput_before: self.throughput,
            throughput_min: self.throughput,
            throughput_delta: 0.0,
            recovered: false,
            recovery_tick: None,
        });
    }

    // ── Pure math helpers (exposed for tests) ─────────────────────────────

    /// Recompute MTTR from a slice of recovery durations.
    /// Returns 0.0 if the slice is empty.
    pub fn compute_mttr(recovery_times: &[u64]) -> f32 {
        if recovery_times.is_empty() {
            return 0.0;
        }
        let sum: u64 = recovery_times.iter().sum();
        sum as f32 / recovery_times.len() as f32
    }

    /// Compute recovery rate as recovered / affected.
    /// Returns 0.0 if `total_affected` is 0.
    pub fn compute_recovery_rate(total_recovered: u32, total_affected: u32) -> f32 {
        if total_affected == 0 {
            return 0.0;
        }
        total_recovered as f32 / total_affected as f32
    }

    /// Compute average cascade spread from a slice of per-fault agent counts.
    /// Returns 0.0 if the slice is empty.
    pub fn compute_avg_cascade_spread(spreads: &[u32]) -> f32 {
        if spreads.is_empty() {
            return 0.0;
        }
        let sum: u32 = spreads.iter().sum();
        sum as f32 / spreads.len() as f32
    }

    /// Compute throughput as rolling mean of a window of per-tick arrivals.
    /// Returns 0.0 if the window is empty.
    pub fn compute_throughput(window: &std::collections::VecDeque<u32>) -> f32 {
        if window.is_empty() {
            return 0.0;
        }
        let sum: u32 = window.iter().sum();
        sum as f32 / window.len() as f32
    }

    /// Compute idle ratio as wait_actions / total_actions.
    /// Returns 0.0 if `total_actions` is 0.
    pub fn compute_idle_ratio(wait_actions: u32, total_actions: u32) -> f32 {
        if total_actions == 0 {
            return 0.0;
        }
        wait_actions as f32 / total_actions as f32
    }

    /// Compute MTBF (Mean Time Between Faults) from fault event ticks.
    /// Requires 2+ events. Returns None with 0 or 1 events. (Or 2025)
    pub fn compute_mtbf(event_ticks: &[u64]) -> Option<f32> {
        if event_ticks.len() < 2 {
            return None;
        }
        let mut intervals = Vec::with_capacity(event_ticks.len() - 1);
        for i in 1..event_ticks.len() {
            intervals.push(event_ticks[i].saturating_sub(event_ticks[i - 1]));
        }
        let sum: u64 = intervals.iter().sum();
        Some(sum as f32 / intervals.len() as f32)
    }

    /// Compute Propagation Rate: average fraction of fleet affected per fault.
    /// Each entry is (agents_affected, alive_at_event). Returns 0.0 if empty.
    pub fn compute_propagation_rate(events: &[(u32, u32)]) -> f32 {
        if events.is_empty() {
            return 0.0;
        }
        let sum: f32 = events
            .iter()
            .map(|&(affected, alive)| {
                if alive == 0 { 0.0 } else { affected as f32 / alive as f32 }
            })
            .sum();
        sum / events.len() as f32
    }
}

// ---------------------------------------------------------------------------
// System 1: register_fault_recovery
// ---------------------------------------------------------------------------

pub fn register_fault_recovery(
    cascade: Res<CascadeState>,
    sim_config: Res<SimulationConfig>,
    mut fault_metrics: ResMut<FaultMetrics>,
) {
    let fault_log = &cascade.fault_log;
    let already_processed = fault_metrics.last_fault_log_len;

    for entry in fault_log.iter().skip(already_processed) {
        fault_metrics.cascade_spreads.push(entry.agents_affected);

        // Create FaultEventRecord
        let event_id = fault_metrics.event_records.len();
        let tp = fault_metrics.throughput;
        fault_metrics.event_records.push(FaultEventRecord {
            id: event_id,
            tick: entry.tick,
            fault_type: entry.fault_type,
            source: entry.source,
            position: entry.position,
            agents_affected: entry.agents_affected,
            cascade_depth: entry.max_depth,
            throughput_before: tp,
            throughput_min: tp,
            throughput_delta: 0.0,
            recovered: false,
            recovery_tick: None,
        });

        for (&entity, record) in &cascade.records {
            if entity == entry.faulted_entity {
                continue;
            }
            if record.fault_origin != entry.faulted_entity {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = fault_metrics.recovery_pending.entry(entity) {
                e.insert(sim_config.tick);
                fault_metrics.total_affected += 1;
            }
        }
    }

    fault_metrics.last_fault_log_len = fault_log.len();

    if !fault_metrics.cascade_spreads.is_empty() {
        let sum: u32 = fault_metrics.cascade_spreads.iter().sum();
        fault_metrics.avg_cascade_spread =
            sum as f32 / fault_metrics.cascade_spreads.len() as f32;
    }

    // --- MTBF: mean time between fault events (Or 2025) ---
    let event_ticks: Vec<u64> = fault_metrics.event_records.iter().map(|r| r.tick).collect();
    fault_metrics.mtbf = FaultMetrics::compute_mtbf(&event_ticks);

    // --- Propagation Rate: normalized cascade spread ---
    if !fault_metrics.event_records.is_empty() && fault_metrics.initial_agent_count > 0 {
        let events: Vec<(u32, u32)> = fault_metrics
            .event_records
            .iter()
            .map(|r| (r.agents_affected, fault_metrics.initial_agent_count))
            .collect();
        fault_metrics.propagation_rate = FaultMetrics::compute_propagation_rate(&events);
    }
}

// ---------------------------------------------------------------------------
// System 2: update_fault_metrics (merged with track_agent_actions)
// ---------------------------------------------------------------------------

pub fn update_fault_metrics(
    mut agents: Query<(Entity, &LogicalAgent, &LastAction, Option<&mut AgentActionStats>), Without<Dead>>,
    mut dead_agents: Query<Option<&mut AgentActionStats>, With<Dead>>,
    all_agents: Query<Entity, With<LogicalAgent>>,
    sim_config: Res<SimulationConfig>,
    mut fault_metrics: ResMut<FaultMetrics>,
    mut pending_scratch: Local<Vec<Entity>>,
    mut recovered_scratch: Local<Vec<Entity>>,
) {
    // Initialize agent count on first tick
    if fault_metrics.initial_agent_count == 0 {
        fault_metrics.initial_agent_count = all_agents.iter().count() as u32;
    }

    // --- Track agent actions + throughput + idle ratio in single pass ---
    let mut new_arrivals = 0u32;
    let mut total_actions_sum = 0u32;
    let mut wait_actions_sum = 0u32;
    let mut alive_count = 0u32;

    for (entity, agent, last_action, stats_opt) in &mut agents {
        alive_count += 1;
        // Track action stats inline (was track_agent_actions)
        if let Some(mut stats) = stats_opt {
            stats.total_actions += 1;
            match last_action.0 {
                Action::Wait => stats.wait_actions += 1,
                _ => stats.move_actions += 1,
            }
            total_actions_sum += stats.total_actions;
            wait_actions_sum += stats.wait_actions;
        }

        // Throughput: count new goal arrivals.
        // In lifelong mode, agents get new goals after reaching old ones,
        // so we must re-track them when they leave their goal.
        if agent.has_reached_goal() {
            if !fault_metrics.finished_entities.contains(&entity) {
                fault_metrics.finished_entities.insert(entity);
                new_arrivals += 1;
            }
        } else {
            fault_metrics.finished_entities.remove(&entity);
        }
    }

    fault_metrics.throughput_window.push_back(new_arrivals);
    if fault_metrics.throughput_window.len() > constants::THROUGHPUT_WINDOW_SIZE {
        fault_metrics.throughput_window.pop_front();
    }

    if !fault_metrics.throughput_window.is_empty() {
        let sum: u32 = fault_metrics.throughput_window.iter().sum();
        fault_metrics.throughput = sum as f32 / fault_metrics.throughput_window.len() as f32;
    }

    // --- MTTR: check recovery_pending ---
    // Reuse Local scratch buffers to avoid per-tick heap allocations
    pending_scratch.clear();
    pending_scratch.extend(fault_metrics.recovery_pending.keys().copied());
    recovered_scratch.clear();
    for entity in pending_scratch.iter() {
        let fault_tick = fault_metrics.recovery_pending[entity];
        if let Ok((_, agent, _, _)) = agents.get(*entity)
            && (agent.has_plan() || agent.has_reached_goal()) {
                let recovery_duration = sim_config.tick.saturating_sub(fault_tick);
                fault_metrics.recovery_times.push(recovery_duration);
                fault_metrics.total_recovered += 1;
                recovered_scratch.push(*entity);
            }
    }
    for entity in recovered_scratch.iter() {
        fault_metrics.recovery_pending.remove(entity);
    }

    if !fault_metrics.recovery_times.is_empty() {
        let sum: u64 = fault_metrics.recovery_times.iter().sum();
        fault_metrics.mttr = sum as f32 / fault_metrics.recovery_times.len() as f32;
    }

    // --- Recovery rate ---
    if fault_metrics.total_affected > 0 {
        fault_metrics.recovery_rate =
            fault_metrics.total_recovered as f32 / fault_metrics.total_affected as f32;
    }

    // --- Update FaultEventRecords (throughput min tracking) ---
    // Recovery is now handled by BaselineDiff.recovery_tick (baseline-differential).
    // We still track throughput_min per event for the fault timeline display.
    let current_tp = fault_metrics.throughput;
    for record in &mut fault_metrics.event_records {
        if current_tp < record.throughput_min {
            record.throughput_min = current_tp;
        }
        record.throughput_delta = record.throughput_before - record.throughput_min;
    }

    // --- Idle ratio: dead agents count as waiting every tick ---
    for mut stats in (&mut dead_agents).into_iter().flatten() {
        stats.total_actions += 1;
        stats.wait_actions += 1;
        total_actions_sum += stats.total_actions;
        wait_actions_sum += stats.wait_actions;
    }

    if total_actions_sum > 0 {
        fault_metrics.idle_ratio = wait_actions_sum as f32 / total_actions_sum as f32;
    }

    // --- Survival rate (uses alive_count from main pass) ---
    if fault_metrics.initial_agent_count > 0 {
        let rate = alive_count as f32 / fault_metrics.initial_agent_count as f32;
        fault_metrics
            .survival_series
            .push_back((sim_config.tick, rate));
        if fault_metrics.survival_series.len() > constants::MAX_SURVIVAL_ENTRIES {
            fault_metrics.survival_series.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── FaultMetrics::default ─────────────────────────────────────────────

    #[test]
    fn fault_event_record_lifecycle() {
        let mut record = FaultEventRecord {
            id: 0,
            tick: 100,
            fault_type: FaultType::Breakdown,
            source: FaultSource::Manual,
            position: IVec2::new(5, 3),
            agents_affected: 4,
            cascade_depth: 2,
            throughput_before: 1.0,
            throughput_min: 1.0,
            throughput_delta: 0.0,
            recovered: false,
            recovery_tick: None,
        };

        // Simulate throughput dip
        record.throughput_min = 0.5;
        record.throughput_delta = record.throughput_before - record.throughput_min;
        assert!((record.throughput_delta - 0.5).abs() < 1e-5);

        // Simulate recovery
        record.recovered = true;
        record.recovery_tick = Some(120);
        assert!(record.recovered);
        assert_eq!(record.recovery_tick, Some(120));
    }

    #[test]
    fn fault_event_record_source_and_type() {
        let record = FaultEventRecord {
            id: 1,
            tick: 50,
            fault_type: FaultType::Latency,
            source: FaultSource::Manual,
            position: IVec2::ZERO,
            agents_affected: 0,
            cascade_depth: 0,
            throughput_before: 0.0,
            throughput_min: 0.0,
            throughput_delta: 0.0,
            recovered: false,
            recovery_tick: None,
        };
        assert_eq!(record.fault_type, FaultType::Latency);
        assert_eq!(record.source, FaultSource::Manual);
    }

    #[test]
    fn fault_metrics_default_zero_values() {
        let fm = FaultMetrics::default();
        assert_eq!(fm.mttr, 0.0);
        assert_eq!(fm.total_affected, 0);
        assert_eq!(fm.total_recovered, 0);
        assert_eq!(fm.recovery_rate, 0.0);
        assert_eq!(fm.avg_cascade_spread, 0.0);
        assert_eq!(fm.throughput, 0.0);
        assert_eq!(fm.idle_ratio, 0.0);
        assert_eq!(fm.initial_agent_count, 0);
        assert!(fm.event_records.is_empty());
        assert!(fm.survival_series.is_empty());
    }

    #[test]
    fn fault_metrics_clear_resets_to_default() {
        let mut fm = FaultMetrics::default();
        fm.mttr = 42.0;
        fm.total_affected = 10;
        fm.total_recovered = 7;
        fm.recovery_rate = 0.7;
        fm.avg_cascade_spread = 3.5;
        fm.throughput = 1.2;
        fm.idle_ratio = 0.3;
        fm.initial_agent_count = 50;

        fm.clear();

        assert_eq!(fm.mttr, 0.0);
        assert_eq!(fm.total_affected, 0);
        assert_eq!(fm.total_recovered, 0);
        assert_eq!(fm.recovery_rate, 0.0);
        assert_eq!(fm.avg_cascade_spread, 0.0);
        assert_eq!(fm.throughput, 0.0);
        assert_eq!(fm.idle_ratio, 0.0);
        assert_eq!(fm.initial_agent_count, 0);
    }

    #[test]
    fn fault_metrics_clear_twice_is_safe() {
        let mut fm = FaultMetrics::default();
        fm.mttr = 5.0;
        fm.clear();
        fm.clear();
        assert_eq!(fm.mttr, 0.0);
    }

    // ── compute_mttr ──────────────────────────────────────────────────────

    #[test]
    fn compute_mttr_empty_returns_zero() {
        assert_eq!(FaultMetrics::compute_mttr(&[]), 0.0);
    }

    #[test]
    fn compute_mttr_single_value() {
        assert_eq!(FaultMetrics::compute_mttr(&[10]), 10.0);
    }

    #[test]
    fn compute_mttr_multiple_values() {
        // (5 + 10 + 15) / 3 = 10.0
        let result = FaultMetrics::compute_mttr(&[5, 10, 15]);
        assert!((result - 10.0).abs() < 1e-5, "expected 10.0, got {result}");
    }

    #[test]
    fn compute_mttr_identical_values() {
        let result = FaultMetrics::compute_mttr(&[4, 4, 4, 4]);
        assert!((result - 4.0).abs() < 1e-5, "expected 4.0, got {result}");
    }

    #[test]
    fn compute_mttr_large_values() {
        // 1000 + 2000 = 3000, /2 = 1500
        let result = FaultMetrics::compute_mttr(&[1000, 2000]);
        assert!((result - 1500.0).abs() < 1e-3, "expected 1500.0, got {result}");
    }

    // ── compute_recovery_rate ─────────────────────────────────────────────

    #[test]
    fn compute_recovery_rate_zero_affected_returns_zero() {
        assert_eq!(FaultMetrics::compute_recovery_rate(0, 0), 0.0);
    }

    #[test]
    fn compute_recovery_rate_all_recovered() {
        let rate = FaultMetrics::compute_recovery_rate(10, 10);
        assert!((rate - 1.0).abs() < 1e-5, "expected 1.0, got {rate}");
    }

    #[test]
    fn compute_recovery_rate_half_recovered() {
        let rate = FaultMetrics::compute_recovery_rate(5, 10);
        assert!((rate - 0.5).abs() < 1e-5, "expected 0.5, got {rate}");
    }

    #[test]
    fn compute_recovery_rate_none_recovered() {
        let rate = FaultMetrics::compute_recovery_rate(0, 8);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn compute_recovery_rate_is_clamped_by_denominator() {
        // recovered can't exceed affected in normal operation
        let rate = FaultMetrics::compute_recovery_rate(3, 7);
        assert!(rate < 1.0);
        assert!(rate > 0.0);
    }

    // ── compute_avg_cascade_spread ────────────────────────────────────────

    #[test]
    fn compute_avg_cascade_spread_empty_returns_zero() {
        assert_eq!(FaultMetrics::compute_avg_cascade_spread(&[]), 0.0);
    }

    #[test]
    fn compute_avg_cascade_spread_single_entry() {
        assert_eq!(FaultMetrics::compute_avg_cascade_spread(&[6]), 6.0);
    }

    #[test]
    fn compute_avg_cascade_spread_multiple_entries() {
        // (2 + 4 + 6) / 3 = 4.0
        let result = FaultMetrics::compute_avg_cascade_spread(&[2, 4, 6]);
        assert!((result - 4.0).abs() < 1e-5, "expected 4.0, got {result}");
    }

    #[test]
    fn compute_avg_cascade_spread_with_zeros() {
        // (0 + 0 + 6) / 3 = 2.0
        let result = FaultMetrics::compute_avg_cascade_spread(&[0, 0, 6]);
        assert!((result - 2.0).abs() < 1e-5, "expected 2.0, got {result}");
    }

    // ── compute_throughput ────────────────────────────────────────────────

    #[test]
    fn compute_throughput_empty_window_returns_zero() {
        let window = std::collections::VecDeque::new();
        assert_eq!(FaultMetrics::compute_throughput(&window), 0.0);
    }

    #[test]
    fn compute_throughput_single_tick() {
        let mut window = std::collections::VecDeque::new();
        window.push_back(5u32);
        assert_eq!(FaultMetrics::compute_throughput(&window), 5.0);
    }

    #[test]
    fn compute_throughput_multiple_ticks() {
        let mut window = std::collections::VecDeque::new();
        window.push_back(2u32);
        window.push_back(4u32);
        window.push_back(6u32);
        // (2 + 4 + 6) / 3 = 4.0
        let result = FaultMetrics::compute_throughput(&window);
        assert!((result - 4.0).abs() < 1e-5, "expected 4.0, got {result}");
    }

    #[test]
    fn compute_throughput_all_zero_ticks() {
        let mut window = std::collections::VecDeque::new();
        window.push_back(0u32);
        window.push_back(0u32);
        assert_eq!(FaultMetrics::compute_throughput(&window), 0.0);
    }

    // ── compute_idle_ratio ────────────────────────────────────────────────

    #[test]
    fn compute_idle_ratio_no_actions_returns_zero() {
        assert_eq!(FaultMetrics::compute_idle_ratio(0, 0), 0.0);
    }

    #[test]
    fn compute_idle_ratio_all_idle() {
        let ratio = FaultMetrics::compute_idle_ratio(10, 10);
        assert!((ratio - 1.0).abs() < 1e-5, "expected 1.0, got {ratio}");
    }

    #[test]
    fn compute_idle_ratio_no_idle() {
        let ratio = FaultMetrics::compute_idle_ratio(0, 10);
        assert_eq!(ratio, 0.0);
    }

    #[test]
    fn compute_idle_ratio_half_idle() {
        let ratio = FaultMetrics::compute_idle_ratio(5, 10);
        assert!((ratio - 0.5).abs() < 1e-5, "expected 0.5, got {ratio}");
    }

    #[test]
    fn compute_idle_ratio_between_zero_and_one() {
        let ratio = FaultMetrics::compute_idle_ratio(3, 11);
        assert!(ratio > 0.0);
        assert!(ratio < 1.0);
        // 3/11 ≈ 0.2727...
        assert!((ratio - 3.0 / 11.0).abs() < 1e-5);
    }

    // ── compute_mtbf ─────────────────────────────────────────────────────

    #[test]
    fn compute_mtbf_empty_returns_none() {
        assert_eq!(FaultMetrics::compute_mtbf(&[]), None);
    }

    #[test]
    fn compute_mtbf_single_event_returns_none() {
        assert_eq!(FaultMetrics::compute_mtbf(&[100]), None);
    }

    #[test]
    fn compute_mtbf_two_events() {
        let result = FaultMetrics::compute_mtbf(&[100, 150]);
        assert_eq!(result, Some(50.0));
    }

    #[test]
    fn compute_mtbf_three_events() {
        // intervals: 50, 30 → mean = 40
        let result = FaultMetrics::compute_mtbf(&[100, 150, 180]);
        assert!((result.unwrap() - 40.0).abs() < 1e-5);
    }

    #[test]
    fn compute_mtbf_same_tick_events() {
        let result = FaultMetrics::compute_mtbf(&[100, 100]);
        assert_eq!(result, Some(0.0));
    }

    // ── compute_propagation_rate ──────────────────────────────────────────

    #[test]
    fn compute_propagation_rate_empty_returns_zero() {
        assert_eq!(FaultMetrics::compute_propagation_rate(&[]), 0.0);
    }

    #[test]
    fn compute_propagation_rate_single_event() {
        // 3 affected out of 20 = 0.15
        let result = FaultMetrics::compute_propagation_rate(&[(3, 20)]);
        assert!((result - 0.15).abs() < 1e-5);
    }

    #[test]
    fn compute_propagation_rate_multiple_events() {
        // (2/10 + 4/10) / 2 = (0.2 + 0.4) / 2 = 0.3
        let result = FaultMetrics::compute_propagation_rate(&[(2, 10), (4, 10)]);
        assert!((result - 0.3).abs() < 1e-5);
    }

    #[test]
    fn compute_propagation_rate_zero_alive_safe() {
        let result = FaultMetrics::compute_propagation_rate(&[(5, 0)]);
        assert_eq!(result, 0.0);
    }
}
