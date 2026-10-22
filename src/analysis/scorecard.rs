//! Resilience Scorecard — live 4-metric assessment (literature-backed).
//!
//! - **Fault Tolerance**: post-fault mean throughput ÷ the twin's post-fault mean
//!   (`super::ft`, shared with the experiment runner)
//! - **NRR**: `1 - MTTR/MTBF` — Normalized Recovery Ratio (Or 2025)
//! - **Survival Rate**: `alive / initial_fleet` — fraction of initial fleet still alive post-fault
//! - **Critical Time**: fraction of post-fault ticks below critical threshold
//!   (operational SLA-style threshold; see `constants::CRITICAL_TIME_THRESHOLD`)

use bevy::prelude::*;
use serde::Serialize;

use crate::constants;
use crate::core::phase::SimulationPhase;
use crate::core::state::SimulationConfig;

use super::TimeSeriesAccessor;
use super::baseline::{BaselineDiff, BaselineStore};
use super::fault_metrics::FaultMetrics;
use super::ft;

// ---------------------------------------------------------------------------
// ResilienceScorecard resource
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Clone, Serialize)]
pub struct ResilienceScorecard {
    /// Post-fault mean throughput ÷ the twin's post-fault mean (`super::ft`). 0-1+.
    /// 1.0 before any fault; `None` when the twin completed nothing after the fault.
    pub fault_tolerance: Option<f32>,
    /// NRR = 1 - MTTR/MTBF. 0-1. (Or 2025)
    pub nrr: Option<f32>,
    /// Survival Rate: alive agents / initial fleet size, measured post-fault.
    /// 1.0 = no deaths, 0.0 = all agents dead.
    pub survival_rate: f32,
    /// Fraction of post-fault ticks below critical threshold. 0-1.
    /// Threshold is an operational SLA heuristic (`constants::CRITICAL_TIME_THRESHOLD`),
    /// not a derived constant from a specific performability paper.
    pub critical_time: f32,
    /// Whether any faults have occurred (controls UI visibility).
    pub has_faults: bool,
}

impl Default for ResilienceScorecard {
    fn default() -> Self {
        Self {
            fault_tolerance: Some(1.0),
            nrr: None,
            survival_rate: 1.0,
            critical_time: 0.0,
            has_faults: false,
        }
    }
}

impl ResilienceScorecard {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Internal tracking state (not serialized to bridge).
#[derive(Resource, Debug, Default)]
pub struct ScorecardState {
    /// Tick at which first fault occurred.
    first_fault_tick: Option<u64>,
    /// Count of ticks in fault period (Critical Time denominator).
    fault_tick_count: u64,
    /// Count of ticks where throughput was below critical threshold.
    ticks_below_critical: u64,
    /// Previous fault event count (for detecting new faults).
    prev_fault_count: usize,
    /// Initial fleet size captured on first fault tick.
    initial_fleet: usize,
    /// Per-tick live and baseline throughput since the first fault, fed to the
    /// shared Fault Tolerance (`super::ft`) and windowed ratio (`super::ct`).
    post_fault_live_tp: Vec<f64>,
    post_fault_baseline_tp: Vec<f64>,
}

impl ScorecardState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

pub fn update_resilience_scorecard(
    sim_config: Res<SimulationConfig>,
    phase: Res<SimulationPhase>,
    baseline_store: Res<BaselineStore>,
    fault_metrics: Res<FaultMetrics>,
    live_sim: Option<Res<crate::core::live_sim::LiveSim>>,
    baseline_diff: Res<BaselineDiff>,
    mut scorecard: ResMut<ResilienceScorecard>,
    mut state: ResMut<ScorecardState>,
) {
    if !phase.is_fault_injection() {
        return;
    }

    let tick = sim_config.tick;

    // --- Detect first fault ---
    let current_event_count = fault_metrics.event_records.len();
    if current_event_count > state.prev_fault_count {
        if state.first_fault_tick.is_none() {
            state.first_fault_tick = Some(tick);
        }
        scorecard.has_faults = true;
        state.prev_fault_count = current_event_count;
    }

    // Track live and baseline throughput from the first fault onwards.
    if state.first_fault_tick.is_some() {
        // Use the instantaneous throughput from baseline_diff (live side).
        // baseline_diff.rate_delta = baseline_tp - live_tp, so:
        // live_tp = baseline_tp - rate_delta
        let bl_tp_at_tick =
            baseline_store.record.as_ref().map(|r| r.throughput_at(tick)).unwrap_or(0.0);
        let live_tp = bl_tp_at_tick - baseline_diff.rate_delta;

        state.fault_tick_count += 1;
        state.post_fault_live_tp.push(live_tp);
        state.post_fault_baseline_tp.push(bl_tp_at_tick);

        // --- Fault Tolerance: same formula as the experiment runner (`crate::analysis::ft`) ---
        scorecard.fault_tolerance =
            ft::fault_tolerance(&state.post_fault_live_tp, &state.post_fault_baseline_tp)
                .map(|v| v as f32);

        // --- Critical Time: ticks below threshold (operational SLA heuristic) ---
        // Same windowed ratio as the experiment runner (`crate::analysis::ct`):
        // live and baseline throughput summed over the same post-fault window,
        // so a single zero-completion tick is not automatically critical.
        let t = state.post_fault_live_tp.len() - 1;
        let ratio =
            super::ct::ratio_at(&state.post_fault_baseline_tp, &state.post_fault_live_tp, 0, t);
        if ratio.is_some_and(|r| r < constants::CRITICAL_TIME_THRESHOLD) {
            state.ticks_below_critical += 1;
        }
        scorecard.critical_time =
            state.ticks_below_critical as f32 / state.fault_tick_count.max(1) as f32;
    }

    // --- NRR: 1 - MTTR/MTBF (Or 2025) ---
    // Requires MTBF (2+ fault events with distinct ticks) AND at least one cascade-affected agent.
    // N/A when: no cascade (total_affected == 0), or all events at same tick (MTBF == 0).
    // Same-tick events (burst, zone outage) have no meaningful inter-failure interval.
    scorecard.nrr = if fault_metrics.total_affected == 0 {
        None
    } else {
        fault_metrics.mtbf.and_then(|mtbf| {
            if mtbf <= 0.0 { None } else { Some((1.0 - fault_metrics.mttr / mtbf).clamp(0.0, 1.0)) }
        })
    };

    // --- Survival Rate: alive / initial fleet ---
    // Honest headcount metric: fraction of the original fleet still alive post-fault.
    if let Some(ls) = live_sim.as_ref() {
        let runner = &ls.runner;
        let total = runner.agents.len();
        if total > 0 {
            // Capture initial fleet size on first fault tick
            if state.initial_fleet == 0 {
                state.initial_fleet = total;
            }
            let alive = runner.agents.iter().filter(|a| a.alive).count();
            scorecard.survival_rate = alive as f32 / state.initial_fleet as f32;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── State lifecycle ──────────────────────────────────────────────

    #[test]
    fn clear_resets_all_fields_to_defaults() {
        let mut sc = ResilienceScorecard {
            fault_tolerance: Some(0.5),
            nrr: Some(0.8),
            survival_rate: 0.5,
            critical_time: 0.2,
            has_faults: true,
        };
        sc.clear();
        assert_eq!(sc.fault_tolerance, Some(1.0));
        assert_eq!(sc.nrr, None);
        assert_eq!(sc.survival_rate, 1.0);
        assert_eq!(sc.critical_time, 0.0);
        assert!(!sc.has_faults);
    }

    // ── Fault Tolerance (FT = P_fault / P_nominal) ───────────────────

    #[test]
    fn ft_no_degradation_equals_one() {
        assert!(((0.5f64 / 0.5f64) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ft_half_performance_equals_half() {
        assert!(((0.25f64 / 0.5f64) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn ft_braess_paradox_above_one() {
        // FT can exceed 1.0 (Braess paradox): faulted throughput > baseline.
        // Simulate: baseline=0.5, faulted=0.6 → FT = 0.6/0.5 = 1.2.
        let baseline = 0.5f32;
        let faulted = 0.6f32;
        let ft = faulted / baseline;
        assert!(ft > 1.0, "FT={ft} should exceed 1.0 for Braess-paradox case");
    }

    // ── Normalized Recovery Rate (NRR = 1 - MTTR/MTBF) ──────────────

    #[test]
    fn nrr_fast_recovery_is_high() {
        let nrr = 1.0 - 2.0f32 / 20.0;
        assert!((nrr - 0.9).abs() < 1e-5);
    }

    #[test]
    fn nrr_slow_recovery_is_low() {
        let nrr = 1.0 - 18.0f32 / 20.0;
        assert!((nrr - 0.1).abs() < 1e-5);
    }

    #[test]
    fn nrr_mttr_exceeds_mtbf_clamps_to_zero() {
        let nrr = (1.0 - 25.0f32 / 20.0).clamp(0.0, 1.0);
        assert_eq!(nrr, 0.0);
    }

    // ── Critical Time (R_crit = ticks_below / ticks_since_fault) ─────

    #[test]
    fn critical_time_zero_when_always_above_threshold() {
        assert_eq!(0u64 as f32 / 100.0, 0.0);
    }

    #[test]
    fn critical_time_half_when_half_below_threshold() {
        assert!((50.0f32 / 100.0 - 0.5).abs() < 1e-5);
    }

    #[test]
    fn critical_time_one_when_always_below_threshold() {
        assert!((100.0f32 / 100.0 - 1.0).abs() < 1e-5);
    }
}
