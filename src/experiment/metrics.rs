//! Headless differential metrics computed from paired baseline/faulted runs.
//!
//! These metrics are the core research outputs: they quantify how a system
//! degrades and recovers under fault conditions compared to a clean baseline.

use crate::analysis::baseline::{BaselineDiff, BaselineRecord};
use crate::analysis::engine::AnalysisEngine;
use crate::analysis::fault_metrics::FaultMetrics;

/// All scalar metrics from a single experiment run.
#[derive(Debug, Clone)]
pub struct RunMetrics {
    // ── Throughput ──────────────────────────────────────────────────
    pub avg_throughput: f64,
    pub total_tasks: u64,

    // ── Agent utilization ──────────────────────────────────────────
    /// Fraction of agent-ticks spent idle: `sum(idle_count[t]) / sum(total_agents[t])`.
    /// An agent is "idle" when its task leg is `Idle` (no active pickup/delivery).
    /// This differs from `wait_ratio` which counts physical `Wait` actions.
    pub idle_ratio: f64,
    /// Cumulative wait ratio: `total_wait_actions / total_actions` across all agent-ticks.
    /// Includes dead agents (permanent `Wait`). Higher = more congestion or faults.
    pub wait_ratio: f64,

    // ── Fault resilience (differential) ────────────────────────────
    /// Fault Tolerance: faulted_avg_tp / baseline_avg_tp (0..1+)
    pub fault_tolerance: f64,
    /// Net Resilience Ratio: 1 - MTTR/MTBF. Higher = more resilient.
    pub nrr: f64,
    /// Fraction of ticks below threshold after first fault.
    pub critical_time: f64,

    // ── Recovery ───────────────────────────────────────────────────
    /// Deficit recovery duration: ticks from first cumulative task deficit
    /// to full catch-up (gap <= 0). Measures total degradation duration,
    /// NOT per-fault recovery time. NaN if deficit never closes.
    pub deficit_recovery: f64,
    /// Throughput recovery: first tick after fault onset where per-tick
    /// faulted throughput >= per-tick baseline throughput. Measures how
    /// quickly the system returns to normal RATE. NaN if never recovers.
    pub throughput_recovery: f64,
    pub mtbf: Option<f64>,
    pub recovery_tick: Option<u64>,

    // ── Cascade / spread ───────────────────────────────────────────
    pub propagation_rate: f64,
    pub survival_rate: f64,
    pub impacted_area: f64,
    pub deficit_integral: i64,

    // ── Performance ────────────────────────────────────────────────
    pub solver_step_time_avg_us: f64,
    pub solver_step_time_max_us: f64,
    pub wall_time_ms: u64,
}

use crate::constants::CRITICAL_TIME_THRESHOLD as CRITICAL_THRESHOLD;

/// Compute differential metrics from a paired baseline + faulted run.
pub fn compute_run_metrics(
    baseline: &BaselineRecord,
    faulted_analysis: &AnalysisEngine,
    faulted_fault_events: &[Vec<crate::core::runner::FaultRecord>],
    solver_step_times_us: &[f64],
    wall_time_ms: u64,
) -> RunMetrics {
    let tick_count = faulted_analysis.tick_count();

    // ── Basic throughput ────────────────────────────────────────────
    let avg_throughput = if tick_count > 0 {
        faulted_analysis.throughput_series.iter().sum::<f64>() / tick_count as f64
    } else {
        0.0
    };
    let total_tasks = faulted_analysis
        .tasks_completed_series
        .last()
        .copied()
        .unwrap_or(0);

    // ── Idle / wait ratio ──────────────────────────────────────────
    // wait_ratio = cumulative (Wait actions / total actions). From AnalysisEngine.
    let wait_ratio = faulted_analysis
        .wait_ratio_series
        .last()
        .copied()
        .unwrap_or(0.0) as f64;

    // idle_ratio = fraction of agent-ticks where the agent had no task assignment
    // (task leg == Idle). This is distinct from wait_ratio which measures physical
    // Wait actions regardless of task state.
    let idle_ratio = if tick_count > 0 {
        let total_idle: usize = faulted_analysis.idle_count_series.iter().sum();
        let total_agents: usize = faulted_analysis
            .alive_series
            .iter()
            .zip(faulted_analysis.dead_series.iter())
            .map(|(a, d)| a + d)
            .sum();
        if total_agents > 0 {
            total_idle as f64 / total_agents as f64
        } else {
            0.0
        }
    } else {
        0.0
    };

    // ── Fault Tolerance (post-fault-onset only) ───────────────────
    // Find first tick where fault events occurred (0-indexed into series).
    let first_fault_idx = faulted_fault_events
        .iter()
        .position(|events| !events.is_empty());

    let fault_tolerance = match first_fault_idx {
        Some(start) => {
            // Average throughput from fault onset to end, for both runs.
            let faulted_post = &faulted_analysis.throughput_series[start..];
            let baseline_post = if start < baseline.throughput_series.len() {
                &baseline.throughput_series[start..]
            } else {
                &baseline.throughput_series[..]
            };
            let avg_faulted: f64 = if faulted_post.is_empty() {
                0.0
            } else {
                faulted_post.iter().sum::<f64>() / faulted_post.len() as f64
            };
            let avg_baseline: f64 = if baseline_post.is_empty() {
                0.0
            } else {
                baseline_post.iter().sum::<f64>() / baseline_post.len() as f64
            };
            if avg_baseline > 0.0 {
                avg_faulted / avg_baseline
            } else {
                f64::NAN
            }
        }
        None => {
            // No faults occurred → faulted run is identical to baseline → FT = 1.0
            if baseline.avg_throughput > 0.0 {
                avg_throughput / baseline.avg_throughput
            } else {
                f64::NAN
            }
        }
    };

    // ── BaselineDiff for differential metrics ──────────────────────
    let mut diff = BaselineDiff::default();
    diff.recompute(
        baseline,
        &faulted_analysis.tasks_completed_series,
        &faulted_analysis.throughput_series,
    );

    // ── MTTR / MTBF from fault events ──────────────────────────────
    let all_fault_ticks: Vec<u64> = faulted_fault_events
        .iter()
        .enumerate()
        .flat_map(|(i, events)| {
            if events.is_empty() {
                vec![]
            } else {
                vec![i as u64 + 1]
            }
        })
        .collect();

    let mtbf = FaultMetrics::compute_mtbf(&all_fault_ticks).map(|v| v as f64);

    // Deficit recovery: ticks from first cumulative deficit to full catch-up.
    let deficit_recovery = match (diff.recovery_tick, diff.first_gap_tick) {
        (Some(recovery), Some(first_gap)) if recovery > first_gap => {
            (recovery - first_gap) as f64
        }
        (None, Some(_)) => f64::NAN, // gap occurred but never recovered
        _ => 0.0,                     // no gap = no fault impact = genuinely 0
    };

    // Throughput recovery: first tick after fault onset where per-tick
    // faulted throughput >= per-tick baseline throughput. Measures rate recovery.
    let throughput_recovery = compute_throughput_recovery(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
    );

    // ── NRR ────────────────────────────────────────────────────────
    // Uses throughput_recovery (rate-based) for a more meaningful ratio.
    let nrr = match mtbf {
        Some(mtbf_val) if mtbf_val > 0.0 => {
            if throughput_recovery.is_nan() {
                f64::NAN // never recovered → NRR undefined
            } else {
                1.0 - (throughput_recovery / mtbf_val)
            }
        }
        _ => {
            // MTBF undefined (< 2 fault events): NRR is a fault-recovery metric,
            // so it's undefined when there aren't enough fault events to measure.
            f64::NAN
        }
    };

    // ── Critical Time ──────────────────────────────────────────────
    // Use first_fault_idx (direct from fault events) instead of first_gap_tick
    // (which is based on cumulative task deficit — can lag behind actual fault onset).
    let critical_time = compute_critical_time(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx.map(|i| i as u64 + 1), // convert 0-indexed to 1-indexed tick
    );

    // ── Survival rate (final) ──────────────────────────────────────
    let survival_rate = if tick_count > 0 {
        let final_alive = faulted_analysis.alive_series.last().copied().unwrap_or(0);
        let total = faulted_analysis.alive_series.first().copied().unwrap_or(0)
            + faulted_analysis.dead_series.first().copied().unwrap_or(0);
        if total > 0 {
            final_alive as f64 / total as f64
        } else {
            1.0
        }
    } else {
        1.0
    };

    // ── Propagation rate from fault events ─────────────────────────
    // Use alive count from the PREVIOUS tick (before this tick's faults fired).
    // alive_series[tick_idx] is recorded AFTER the tick completes, so it already
    // reflects agents killed in that tick. Using it as the denominator would
    // undercount the population at risk (e.g., 5 faults out of 20 agents would
    // read as 5/15 = 0.33 instead of the correct 5/20 = 0.25).
    // For tick 0 there is no previous tick, so we use the initial fleet size.
    let propagation_events: Vec<(u32, u32)> = faulted_fault_events
        .iter()
        .enumerate()
        .filter(|(_, events)| !events.is_empty())
        .map(|(tick_idx, events)| {
            let alive_before_fault = if tick_idx > 0 {
                faulted_analysis.alive_series[tick_idx - 1] as u32
            } else {
                // First tick: use initial fleet size (all alive)
                let total = faulted_analysis.alive_series.first().copied().unwrap_or(0)
                    + faulted_analysis.dead_series.first().copied().unwrap_or(0);
                total as u32
            };
            (events.len() as u32, alive_before_fault)
        })
        .collect();
    let propagation_rate = FaultMetrics::compute_propagation_rate(&propagation_events) as f64;

    // ── Solver step timing ─────────────────────────────────────────
    let solver_step_time_avg_us = if solver_step_times_us.is_empty() {
        0.0
    } else {
        solver_step_times_us.iter().sum::<f64>() / solver_step_times_us.len() as f64
    };
    let solver_step_time_max_us = solver_step_times_us
        .iter()
        .copied()
        .fold(0.0_f64, f64::max);

    RunMetrics {
        avg_throughput,
        total_tasks,
        idle_ratio,
        wait_ratio,
        fault_tolerance,
        nrr,
        critical_time,
        deficit_recovery,
        throughput_recovery,
        mtbf,
        recovery_tick: diff.recovery_tick,
        propagation_rate,
        survival_rate,
        impacted_area: diff.impacted_area,
        deficit_integral: diff.deficit_integral,
        solver_step_time_avg_us,
        solver_step_time_max_us,
        wall_time_ms,
    }
}

/// Compute throughput recovery: number of ticks from fault onset until per-tick
/// faulted throughput >= per-tick baseline throughput.
///
/// Returns 0.0 if no faults occurred, NaN if throughput never recovers.
/// This measures rate recovery (how quickly the system returns to normal throughput),
/// not cumulative deficit recovery.
fn compute_throughput_recovery(
    baseline_tp: &[f64],
    faulted_tp: &[f64],
    first_fault_idx: Option<usize>,
) -> f64 {
    let start = match first_fault_idx {
        Some(s) => s,
        None => return 0.0, // no faults → no recovery needed
    };

    let len = baseline_tp.len().min(faulted_tp.len());
    if start >= len {
        return 0.0;
    }

    // Find first tick AFTER fault onset where faulted >= baseline.
    // Skip the fault tick itself (throughput may drop to 0 on the fault tick).
    for i in (start + 1)..len {
        if faulted_tp[i] >= baseline_tp[i] {
            return (i - start) as f64;
        }
    }

    f64::NAN // never recovered
}

/// Compute fraction of ticks where faulted throughput < threshold × baseline throughput,
/// counted from the first fault tick onward.
fn compute_critical_time(
    baseline_tp: &[f64],
    faulted_tp: &[f64],
    first_gap_tick: Option<u64>,
) -> f64 {
    let start = match first_gap_tick {
        Some(t) if t > 0 => (t - 1) as usize, // convert 1-indexed tick to 0-indexed
        Some(_) => return 0.0,                  // tick 0 edge case
        None => return f64::NAN,                // no fault impact → metric undefined
    };

    let len = baseline_tp.len().min(faulted_tp.len());
    if start >= len {
        return 0.0;
    }

    let ticks_after_fault = len - start;
    let ticks_below = (start..len)
        .filter(|&i| {
            let threshold = baseline_tp[i] * CRITICAL_THRESHOLD;
            faulted_tp[i] < threshold
        })
        .count();

    if ticks_after_fault > 0 {
        ticks_below as f64 / ticks_after_fault as f64
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_time_no_fault() {
        // No fault impact → metric is undefined (NaN), not zero
        assert!(compute_critical_time(&[1.0; 10], &[1.0; 10], None).is_nan());
    }

    #[test]
    fn critical_time_all_below() {
        // Baseline all 2.0, faulted all 0.0 after tick 3
        let bl = vec![2.0; 10];
        let mut faulted = vec![2.0; 10];
        for i in 2..10 {
            faulted[i] = 0.0;
        }
        let ct = compute_critical_time(&bl, &faulted, Some(3));
        // 8 ticks after fault, all below 50% threshold
        assert!((ct - 1.0).abs() < 1e-10);
    }

    #[test]
    fn critical_time_half_below() {
        let bl = vec![2.0; 10];
        let mut faulted = vec![2.0; 10];
        // First 4 ticks after fault: below threshold
        // Next 4 ticks: above threshold
        for i in 2..6 {
            faulted[i] = 0.0;
        }
        let ct = compute_critical_time(&bl, &faulted, Some(3));
        // 8 ticks after fault, 4 below
        assert!((ct - 0.5).abs() < 1e-10);
    }
}
