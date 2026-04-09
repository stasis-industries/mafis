//! Fault Resilience Metrics Pipeline
//!
//! Computes research-grade KPIs for fault resilience analysis:
//! - MTTR (Mean Time To Recovery)
//! - Recovery rate
//! - Cascade spread
//! - Wait ratio (aggregate)
//! - Fault survival rate (time-series)

use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

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
    pub recovered: bool,
    pub recovery_tick: Option<u64>,
    /// Alive agent count at the time this fault event occurred.
    /// Used as denominator for propagation rate (instead of initial fleet size).
    pub alive_at_event: u32,
}

// ---------------------------------------------------------------------------
// FaultMetrics resource
// ---------------------------------------------------------------------------

#[derive(Resource, Debug)]
pub struct FaultMetrics {
    recovery_pending: HashMap<Entity, u64>,
    /// Bounded sliding window of recovery durations. Capped at
    /// `MAX_FAULT_EVENT_HISTORY` to prevent unbounded growth in long
    /// fault-heavy simulations. Statistical accuracy across the full
    /// simulation is preserved by `recovery_times_sum` / `recovery_times_count`.
    recovery_times: VecDeque<u64>,
    pub mttr: f32,

    pub total_affected: u32,
    pub total_recovered: u32,
    pub recovery_rate: f32,

    /// Bounded sliding window of cascade spreads (one entry per fault event).
    /// Same capping + running-aggregate strategy as `recovery_times`.
    cascade_spreads: VecDeque<u32>,
    pub avg_cascade_spread: f32,

    /// Mean Time Between Faults — average ticks between consecutive fault events.
    /// Requires 2+ events. (Or 2025)
    pub mtbf: Option<f32>,
    /// Propagation Rate — avg(agents_affected / alive_at_event) across all faults.
    /// Normalized 0-1 fraction of fleet affected per event.
    pub propagation_rate: f32,

    pub wait_ratio: f32,

    pub survival_series: VecDeque<(u64, f32)>,
    pub initial_agent_count: u32,

    /// Bounded sliding window of fault event records. Capped at
    /// `MAX_FAULT_EVENT_HISTORY`. The bridge serializer takes only the last
    /// 100 events per sync (`bridge/serialize.rs:678`); the desktop UI takes
    /// only the last 10 (`ui/desktop/panels/fault_response.rs:66`); the
    /// scorecard reads `len()` only. So a 1000-entry cap is plenty.
    pub event_records: VecDeque<FaultEventRecord>,

    last_fault_log_len: usize,

    // ─── Running aggregates (decoupled from storage cap) ────────────────
    //
    // These accumulate over the *entire* simulation, even when old entries
    // are evicted from the bounded VecDeques above. This preserves the
    // statistical accuracy of derived metrics (avg cascade spread, MTTR,
    // MTBF, propagation rate) regardless of how many events have been seen.
    //
    // Updated incrementally on each push (O(1)), never recomputed by walking
    // the full history. Reset on `clear()` and `recompute_aggregates_from_remaining()`.
    cascade_spreads_sum: u64,
    cascade_spreads_count: u64,
    recovery_times_sum: u64,
    recovery_times_count: u64,
    mtbf_last_tick: Option<u64>,
    mtbf_intervals_sum: u64,
    mtbf_intervals_count: u64,
    propagation_rate_sum: f32,
    propagation_rate_count: u64,
}

impl Default for FaultMetrics {
    fn default() -> Self {
        Self {
            recovery_pending: HashMap::new(),
            recovery_times: VecDeque::with_capacity(constants::MAX_FAULT_EVENT_HISTORY + 1),
            mttr: 0.0,
            total_affected: 0,
            total_recovered: 0,
            recovery_rate: 0.0,
            cascade_spreads: VecDeque::with_capacity(constants::MAX_FAULT_EVENT_HISTORY + 1),
            avg_cascade_spread: 0.0,
            mtbf: None,
            propagation_rate: 0.0,
            wait_ratio: 0.0,
            survival_series: VecDeque::with_capacity(constants::MAX_SURVIVAL_ENTRIES + 1),
            initial_agent_count: 0,
            event_records: VecDeque::with_capacity(constants::MAX_FAULT_EVENT_HISTORY + 1),
            last_fault_log_len: 0,
            cascade_spreads_sum: 0,
            cascade_spreads_count: 0,
            recovery_times_sum: 0,
            recovery_times_count: 0,
            mtbf_last_tick: None,
            mtbf_intervals_sum: 0,
            mtbf_intervals_count: 0,
            propagation_rate_sum: 0.0,
            propagation_rate_count: 0,
        }
    }
}

impl FaultMetrics {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Truncate all data after `tick` for rewind support. After evicting
    /// entries that no longer fit the rewind tick, the running aggregates
    /// must be rebuilt from the remaining entries to keep derived metrics
    /// (MTTR, avg cascade spread, MTBF, propagation rate) consistent with
    /// the post-rewind state.
    pub fn truncate_after_tick(&mut self, tick: u64) {
        self.event_records.retain(|r| r.tick <= tick);
        self.survival_series.retain(|&(t, _)| t <= tick);
        // We can't easily filter cascade_spreads/recovery_times by tick
        // (they don't carry per-entry tick metadata), so on rewind we
        // truncate them along with event_records by recomputing from the
        // remaining events. This loses recovery durations and intermediate
        // event metrics for the evicted suffix — acceptable for rewind which
        // already drops live state.
        self.cascade_spreads.clear();
        for r in &self.event_records {
            self.cascade_spreads.push_back(r.agents_affected);
        }
        self.recompute_aggregates_from_remaining();
        // Will be recalculated by register_fault_recovery
        self.last_fault_log_len = 0;
    }

    /// Recompute every running aggregate from whatever entries are still
    /// present in the bounded VecDeques. Called after `truncate_after_tick`
    /// to restore consistency post-rewind. Intentionally O(remaining) — this
    /// is the only path that walks the full history; the steady-state push
    /// path is O(1).
    fn recompute_aggregates_from_remaining(&mut self) {
        // cascade_spreads
        self.cascade_spreads_sum = self.cascade_spreads.iter().map(|&x| x as u64).sum();
        self.cascade_spreads_count = self.cascade_spreads.len() as u64;

        // recovery_times — note: these are NOT regenerated from event_records
        // because they don't 1:1 correspond. We just use whatever remains.
        self.recovery_times_sum = self.recovery_times.iter().sum();
        self.recovery_times_count = self.recovery_times.len() as u64;

        // MTBF — walk event_records ticks
        let ticks: Vec<u64> = self.event_records.iter().map(|r| r.tick).collect();
        self.mtbf_last_tick = ticks.last().copied();
        self.mtbf_intervals_sum = 0;
        self.mtbf_intervals_count = 0;
        for w in ticks.windows(2) {
            self.mtbf_intervals_sum += w[1].saturating_sub(w[0]);
            self.mtbf_intervals_count += 1;
        }

        // Propagation rate
        self.propagation_rate_sum = self
            .event_records
            .iter()
            .map(|r| {
                if r.alive_at_event > 0 {
                    r.agents_affected as f32 / r.alive_at_event as f32
                } else {
                    0.0
                }
            })
            .sum();
        self.propagation_rate_count = self.event_records.len() as u64;
    }

    /// Push a fault event record onto the bounded deque, evicting the oldest
    /// entry if at capacity. Returns the assigned event id (which is
    /// monotonically increasing across the *full* simulation, not just the
    /// current window — same as before).
    fn push_event_record(&mut self, record: FaultEventRecord) {
        self.event_records.push_back(record);
        while self.event_records.len() > constants::MAX_FAULT_EVENT_HISTORY {
            self.event_records.pop_front();
        }
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
        alive_count: u32,
    ) {
        // The event id is the *cumulative* count, NOT `event_records.len()`,
        // so it stays monotonic even after the deque evicts old entries.
        let id = self.cascade_spreads_count as usize;
        self.push_event_record(FaultEventRecord {
            id,
            tick,
            fault_type,
            source,
            position,
            agents_affected: 1,
            cascade_depth: 0,
            recovered: false,
            recovery_tick: None,
            alive_at_event: alive_count,
        });

        // Manual events: agents_affected = 1, treat as a single-cell cascade.
        self.cascade_spreads.push_back(1);
        while self.cascade_spreads.len() > constants::MAX_FAULT_EVENT_HISTORY {
            self.cascade_spreads.pop_front();
        }
        self.cascade_spreads_sum += 1;
        self.cascade_spreads_count += 1;

        // Update MTBF from the new event tick.
        if let Some(prev) = self.mtbf_last_tick {
            self.mtbf_intervals_sum += tick.saturating_sub(prev);
            self.mtbf_intervals_count += 1;
            self.mtbf = Some(self.mtbf_intervals_sum as f32 / self.mtbf_intervals_count as f32);
        }
        self.mtbf_last_tick = Some(tick);

        // Update propagation rate (1/alive_count for manual events).
        if alive_count > 0 {
            self.propagation_rate_sum += 1.0 / alive_count as f32;
            self.propagation_rate_count += 1;
            self.propagation_rate = self.propagation_rate_sum / self.propagation_rate_count as f32;
        }

        // Update avg_cascade_spread incrementally.
        self.avg_cascade_spread =
            self.cascade_spreads_sum as f32 / self.cascade_spreads_count as f32;
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

    /// Compute wait ratio as wait_actions / total_actions.
    /// Returns 0.0 if `total_actions` is 0.
    pub fn compute_wait_ratio(wait_actions: u32, total_actions: u32) -> f32 {
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
            .map(|&(affected, alive)| if alive == 0 { 0.0 } else { affected as f32 / alive as f32 })
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
    alive_agents: Query<Entity, (With<LogicalAgent>, Without<Dead>)>,
    mut fault_metrics: ResMut<FaultMetrics>,
) {
    let fault_log = &cascade.fault_log;
    let already_processed = fault_metrics.last_fault_log_len;

    // Fast path: nothing new to process. Bail before any work.
    if fault_log.len() <= already_processed {
        return;
    }

    // Count alive agents once (used as denominator for propagation rate).
    // This is the alive count at processing time — close enough since cascade
    // processing happens the same tick as the fault event.
    let alive_now = alive_agents.iter().count() as u32;

    for entry in fault_log.iter().skip(already_processed) {
        // Push to bounded deque + update running aggregate (O(1)).
        fault_metrics.cascade_spreads.push_back(entry.agents_affected);
        while fault_metrics.cascade_spreads.len() > constants::MAX_FAULT_EVENT_HISTORY {
            fault_metrics.cascade_spreads.pop_front();
        }
        fault_metrics.cascade_spreads_sum += entry.agents_affected as u64;
        fault_metrics.cascade_spreads_count += 1;

        // Update MTBF interval from the previous event's tick.
        if let Some(prev_tick) = fault_metrics.mtbf_last_tick {
            fault_metrics.mtbf_intervals_sum += entry.tick.saturating_sub(prev_tick);
            fault_metrics.mtbf_intervals_count += 1;
        }
        fault_metrics.mtbf_last_tick = Some(entry.tick);

        // Update propagation rate running aggregate.
        if alive_now > 0 {
            fault_metrics.propagation_rate_sum += entry.agents_affected as f32 / alive_now as f32;
            fault_metrics.propagation_rate_count += 1;
        }

        // The event id is the *cumulative* count (monotonic across the full
        // simulation), not `event_records.len()` which would reset on eviction.
        let event_id = fault_metrics.cascade_spreads_count as usize - 1;
        fault_metrics.push_event_record(FaultEventRecord {
            id: event_id,
            tick: entry.tick,
            fault_type: entry.fault_type,
            source: entry.source,
            position: entry.position,
            agents_affected: entry.agents_affected,
            cascade_depth: entry.max_depth,
            recovered: false,
            recovery_tick: None,
            alive_at_event: alive_now,
        });

        for (&entity, record) in &cascade.records {
            if entity == entry.faulted_entity {
                continue;
            }
            if record.fault_origin != entry.faulted_entity {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(e) =
                fault_metrics.recovery_pending.entry(entity)
            {
                e.insert(sim_config.tick);
                fault_metrics.total_affected += 1;
            }
        }
    }

    fault_metrics.last_fault_log_len = fault_log.len();

    // Recompute derived metrics from the running aggregates (O(1) — no Vec
    // allocation, no iteration over event_records).
    if fault_metrics.cascade_spreads_count > 0 {
        fault_metrics.avg_cascade_spread =
            fault_metrics.cascade_spreads_sum as f32 / fault_metrics.cascade_spreads_count as f32;
    }
    if fault_metrics.mtbf_intervals_count > 0 {
        fault_metrics.mtbf = Some(
            fault_metrics.mtbf_intervals_sum as f32 / fault_metrics.mtbf_intervals_count as f32,
        );
    }
    if fault_metrics.propagation_rate_count > 0 {
        fault_metrics.propagation_rate =
            fault_metrics.propagation_rate_sum / fault_metrics.propagation_rate_count as f32;
    }
}

// ---------------------------------------------------------------------------
// System 2: update_fault_metrics (merged with track_agent_actions)
// ---------------------------------------------------------------------------

pub fn update_fault_metrics(
    mut agents: Query<
        (Entity, &LogicalAgent, &LastAction, Option<&mut AgentActionStats>),
        Without<Dead>,
    >,
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

    // --- Track agent actions + wait ratio in single pass ---
    let mut total_actions_sum = 0u32;
    let mut wait_actions_sum = 0u32;
    let mut alive_count = 0u32;

    for (_entity, _agent, last_action, stats_opt) in &mut agents {
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
    }

    // --- MTTR: check recovery_pending ---
    // Reuse Local scratch buffers to avoid per-tick heap allocations
    pending_scratch.clear();
    pending_scratch.extend(fault_metrics.recovery_pending.keys().copied());
    recovered_scratch.clear();
    let mut new_recoveries_this_tick = false;
    for entity in pending_scratch.iter() {
        let fault_tick = fault_metrics.recovery_pending[entity];
        if let Ok((_, agent, _, _)) = agents.get(*entity)
            && (agent.has_plan() || agent.has_reached_goal())
        {
            let recovery_duration = sim_config.tick.saturating_sub(fault_tick);
            // Push to bounded deque + update running aggregate (O(1)).
            // No more O(|recovery_times|) sum-walk per tick.
            fault_metrics.recovery_times.push_back(recovery_duration);
            while fault_metrics.recovery_times.len() > constants::MAX_FAULT_EVENT_HISTORY {
                fault_metrics.recovery_times.pop_front();
            }
            fault_metrics.recovery_times_sum += recovery_duration;
            fault_metrics.recovery_times_count += 1;
            fault_metrics.total_recovered += 1;
            recovered_scratch.push(*entity);
            new_recoveries_this_tick = true;
        }
    }
    for entity in recovered_scratch.iter() {
        fault_metrics.recovery_pending.remove(entity);
    }

    // Update MTTR from running aggregates only when something changed —
    // avoids O(events) work on ticks with no recovery activity.
    if new_recoveries_this_tick && fault_metrics.recovery_times_count > 0 {
        fault_metrics.mttr =
            fault_metrics.recovery_times_sum as f32 / fault_metrics.recovery_times_count as f32;
    }

    // --- Recovery rate ---
    if fault_metrics.total_affected > 0 {
        fault_metrics.recovery_rate =
            fault_metrics.total_recovered as f32 / fault_metrics.total_affected as f32;
    }

    // --- Wait ratio: alive agents only ---
    // Dead agents are excluded — their fleet loss is captured by survival_rate and
    // fleet_utilization. Counting dead agents as permanently idle would make this
    // metric unresponsive to late-stage events (old deaths dominate the cumulative sum).
    if total_actions_sum > 0 {
        fault_metrics.wait_ratio = wait_actions_sum as f32 / total_actions_sum as f32;
    }

    // --- Survival rate (uses alive_count from main pass) ---
    if fault_metrics.initial_agent_count > 0 {
        let rate = alive_count as f32 / fault_metrics.initial_agent_count as f32;
        fault_metrics.survival_series.push_back((sim_config.tick, rate));
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
            recovered: false,
            recovery_tick: None,
            alive_at_event: 50,
        };

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
            recovered: false,
            recovery_tick: None,
            alive_at_event: 30,
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
        assert_eq!(fm.wait_ratio, 0.0);
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
        fm.wait_ratio = 0.3;
        fm.initial_agent_count = 50;

        fm.clear();

        assert_eq!(fm.mttr, 0.0);
        assert_eq!(fm.total_affected, 0);
        assert_eq!(fm.total_recovered, 0);
        assert_eq!(fm.recovery_rate, 0.0);
        assert_eq!(fm.avg_cascade_spread, 0.0);
        assert_eq!(fm.wait_ratio, 0.0);
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

    // ── Bounded growth + running aggregates (Phase 5 regression) ──────────

    #[test]
    fn event_records_bounded_under_sustained_faults() {
        let mut fm = FaultMetrics::default();
        // Push 5x the cap. The deque must stay ≤ MAX_FAULT_EVENT_HISTORY.
        for i in 0..(constants::MAX_FAULT_EVENT_HISTORY * 5) {
            fm.register_manual_event(
                i as u64,
                FaultType::Breakdown,
                FaultSource::Manual,
                IVec2::ZERO,
                40,
            );
        }
        assert!(
            fm.event_records.len() <= constants::MAX_FAULT_EVENT_HISTORY,
            "event_records grew past cap: {}",
            fm.event_records.len()
        );
        assert!(
            fm.cascade_spreads.len() <= constants::MAX_FAULT_EVENT_HISTORY,
            "cascade_spreads grew past cap: {}",
            fm.cascade_spreads.len()
        );
    }

    #[test]
    fn running_aggregates_track_full_history_after_eviction() {
        let mut fm = FaultMetrics::default();
        let n = constants::MAX_FAULT_EVENT_HISTORY * 3;
        for i in 0..n {
            fm.register_manual_event(
                i as u64,
                FaultType::Breakdown,
                FaultSource::Manual,
                IVec2::ZERO,
                40,
            );
        }
        // The deque is bounded at MAX, but the running counts must reflect
        // the full simulation (3× MAX events were registered).
        assert_eq!(fm.cascade_spreads_count, n as u64);
        // Manual events all have agents_affected=1, so sum equals count.
        assert_eq!(fm.cascade_spreads_sum, n as u64);
        // avg should be 1.0
        assert!((fm.avg_cascade_spread - 1.0).abs() < 1e-5);
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

    // ── compute_wait_ratio ────────────────────────────────────────────────

    #[test]
    fn compute_wait_ratio_no_actions_returns_zero() {
        assert_eq!(FaultMetrics::compute_wait_ratio(0, 0), 0.0);
    }

    #[test]
    fn compute_wait_ratio_all_idle() {
        let ratio = FaultMetrics::compute_wait_ratio(10, 10);
        assert!((ratio - 1.0).abs() < 1e-5, "expected 1.0, got {ratio}");
    }

    #[test]
    fn compute_wait_ratio_no_idle() {
        let ratio = FaultMetrics::compute_wait_ratio(0, 10);
        assert_eq!(ratio, 0.0);
    }

    #[test]
    fn compute_wait_ratio_half_idle() {
        let ratio = FaultMetrics::compute_wait_ratio(5, 10);
        assert!((ratio - 0.5).abs() < 1e-5, "expected 0.5, got {ratio}");
    }

    #[test]
    fn compute_wait_ratio_between_zero_and_one() {
        let ratio = FaultMetrics::compute_wait_ratio(3, 11);
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
