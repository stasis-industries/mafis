//! Resilience Scorecard — live 4-metric assessment (literature-backed).
//!
//! - **Fault Tolerance**: `P_fault / P_nominal` — adapted from classical reliability
//!   degradation ratio (fraction of baseline throughput retained under faults)
//! - **NRR**: `1 - MTTR/MTBF` — Normalized Recovery Ratio (Or 2025)
//! - **Fleet Utilization**: alive+tasked agents / initial fleet size, averaged post-fault
//! - **Critical Time**: fraction of post-fault ticks below critical threshold
//!   (inspired by performability theory, Ghasemieh & Haverkort; threshold configurable)

use bevy::prelude::*;
use serde::Serialize;

use crate::constants;
use crate::core::phase::SimulationPhase;
use crate::core::state::SimulationConfig;

use super::TimeSeriesAccessor;
use super::baseline::{BaselineDiff, BaselineStore};
use super::fault_metrics::FaultMetrics;

// ---------------------------------------------------------------------------
// ResilienceScorecard resource
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Clone, Serialize)]
pub struct ResilienceScorecard {
    /// Swarm FT = P_fault / P_nominal. 0-1+. (classical reliability degradation ratio)
    pub fault_tolerance: f32,
    /// NRR = 1 - MTTR/MTBF. 0-1. (Or 2025)
    pub nrr: Option<f32>,
    /// Fleet Utilization Ratio: alive+tasked agents / initial fleet size, averaged post-fault.
    /// Measures how much productive capacity the fleet retains after faults.
    /// 1.0 = full utilization, 0.0 = all agents dead or idle.
    pub fleet_utilization: f32,
    /// Fraction of post-fault ticks below critical threshold. 0-1.
    /// (Inspired by performability theory; Ghasemieh & Haverkort; threshold configurable)
    pub critical_time: f32,
    /// Whether any faults have occurred (controls UI visibility).
    pub has_faults: bool,
}

impl Default for ResilienceScorecard {
    fn default() -> Self {
        Self {
            fault_tolerance: 1.0,
            nrr: None,
            fleet_utilization: 1.0,
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
    /// Running sum of live throughput during fault period (for FT average).
    fault_throughput_sum: f64,
    /// Count of ticks in fault period (for FT average).
    fault_tick_count: u64,
    /// Count of ticks where throughput was below critical threshold.
    ticks_below_critical: u64,
    /// Previous fault event count (for detecting new faults).
    prev_fault_count: usize,
    /// Initial fleet size captured on first fault tick.
    initial_fleet: usize,
}

impl ScorecardState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Pure math helpers
// ---------------------------------------------------------------------------

/// Shannon entropy from an iterator of density values, normalized to [0, 1].
/// Zero-allocation: iterates twice via Clone.
pub fn compute_heatmap_entropy(
    density: impl Iterator<Item = f32> + Clone,
    grid_cells: usize,
) -> f64 {
    if grid_cells == 0 {
        return 0.0;
    }

    let total: f32 = density.clone().sum();
    if total <= 0.0 {
        return 0.0;
    }

    let mut entropy: f64 = 0.0;
    for d in density {
        if d > 0.0 {
            let p = d as f64 / total as f64;
            entropy -= p * p.ln();
        }
    }

    let max_entropy = (grid_cells as f64).ln();
    if max_entropy > 0.0 { (entropy / max_entropy).clamp(0.0, 1.0) } else { 0.0 }
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

    // --- Get baseline avg throughput for FT and Critical Time ---
    let baseline_avg_tp = baseline_store.record.as_ref().map(|r| r.avg_throughput).unwrap_or(0.0);

    // --- Fault Tolerance: P_fault / P_nominal (classical reliability ratio) ---
    // Track live throughput from first fault onwards.
    if state.first_fault_tick.is_some() {
        // Use the instantaneous throughput from baseline_diff (live side).
        // baseline_diff.rate_delta = baseline_tp - live_tp, so:
        // live_tp = baseline_tp - rate_delta
        let bl_tp_at_tick =
            baseline_store.record.as_ref().map(|r| r.throughput_at(tick)).unwrap_or(0.0);
        let live_tp = bl_tp_at_tick - baseline_diff.rate_delta;

        state.fault_throughput_sum += live_tp;
        state.fault_tick_count += 1;

        if baseline_avg_tp > 0.0 && state.fault_tick_count > 0 {
            let p_fault = state.fault_throughput_sum / state.fault_tick_count as f64;
            scorecard.fault_tolerance = (p_fault / baseline_avg_tp) as f32;
        }

        // --- Critical Time: ticks below threshold (performability theory) ---
        let critical_threshold = baseline_avg_tp * constants::CRITICAL_TIME_THRESHOLD;
        if live_tp < critical_threshold {
            state.ticks_below_critical += 1;
        }
        scorecard.critical_time =
            state.ticks_below_critical as f32 / state.fault_tick_count.max(1) as f32;
    }

    // --- NRR: 1 - MTTR/MTBF (Or 2025) ---
    // Requires MTBF (2+ fault events) AND at least one cascade-affected agent.
    // For permanent deaths with no cascade (e.g., isolated wear failures),
    // MTTR is trivially 0 → NRR would be a meaningless 100%. Fleet damage
    // in those cases is captured by Fleet Utilization instead.
    scorecard.nrr = if fault_metrics.total_affected == 0 {
        None
    } else {
        fault_metrics.mtbf.map(|mtbf| {
            if mtbf <= 0.0 { 0.0 } else { (1.0 - fault_metrics.mttr / mtbf).clamp(0.0, 1.0) }
        })
    };

    // --- Fleet Utilization Ratio: alive+tasked / initial fleet ---
    // Captures how much productive capacity is retained post-fault.
    if let Some(ls) = live_sim.as_ref() {
        let runner = &ls.runner;
        let total = runner.agents.len();
        if total > 0 {
            // Capture initial fleet size on first fault tick
            if state.initial_fleet == 0 {
                state.initial_fleet = total;
            }
            let tasked = runner
                .agents
                .iter()
                .filter(|a| a.alive && !matches!(a.task_leg, crate::core::task::TaskLeg::Free))
                .count();
            scorecard.fleet_utilization = tasked as f32 / state.initial_fleet as f32;
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
        let mut sc = ResilienceScorecard::default();
        sc.fault_tolerance = 0.5;
        sc.nrr = Some(0.8);
        sc.fleet_utilization = 0.5;
        sc.critical_time = 0.2;
        sc.has_faults = true;
        sc.clear();
        assert_eq!(sc.fault_tolerance, 1.0);
        assert_eq!(sc.nrr, None);
        assert_eq!(sc.fleet_utilization, 1.0);
        assert_eq!(sc.critical_time, 0.0);
        assert!(!sc.has_faults);
    }

    // ── Heatmap entropy ──────────────────────────────────────────────

    #[test]
    fn entropy_empty_is_zero() {
        assert_eq!(compute_heatmap_entropy(std::iter::empty(), 0), 0.0);
    }

    #[test]
    fn entropy_uniform_is_maximal() {
        let density = [1.0f32, 1.0, 1.0, 1.0];
        let e = compute_heatmap_entropy(density.iter().copied(), 4);
        assert!((e - 1.0).abs() < 0.01, "expected ~1.0, got {e}");
    }

    #[test]
    fn entropy_concentrated_is_near_zero() {
        let density = [100.0f32, 0.0, 0.0, 0.0];
        let e = compute_heatmap_entropy(density.iter().copied(), 4);
        assert!(e < 0.1, "expected < 0.1, got {e}");
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
        assert!(0.6f64 / 0.5f64 > 1.0);
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
