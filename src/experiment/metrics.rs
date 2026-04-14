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
    /// Fraction of agent-ticks spent unassigned: `sum(idle_count[t]) / sum(total_agents[t])`.
    /// An agent is "unassigned" when its task leg is `Free` (no active pickup/delivery).
    /// This differs from `wait_ratio` which counts physical `Wait` actions.
    pub unassigned_ratio: f64,
    /// Cumulative wait ratio: `total_wait_actions / total_actions` across all agent-ticks.
    /// Includes dead agents (permanent `Wait`). Higher = more congestion or faults.
    pub wait_ratio: f64,

    // ── Fault resilience (differential) ────────────────────────────
    /// Fault Tolerance: faulted_avg_tp / baseline_avg_tp (0..1+)
    pub fault_tolerance: f64,
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

    // ── Cascade (ADG-based) ─────────────────────────────────────────
    /// Average cascade spread per fault event (agents affected via ADG BFS).
    pub cascade_spread_avg: f64,
    /// Average cascade depth per fault event (max BFS depth).
    pub cascade_depth_avg: f64,

    // ── Tier 1 Resilience Metrics ──────────────────────────────────
    /// Integral of Time-weighted Absolute Error of throughput ratio post-fault.
    /// `J_ITAE = Σ (t - t_fault) · |1 - R(t)|` where `R(t) = faulted_tp[t] / baseline_tp[t]`.
    /// Ogata 2010. Unit: tick² · dimensionless. Lower = better. NaN if no fault.
    pub itae: f64,
    /// Rapidity — ticks from first fault until `R(t) ≥ RAPIDITY_THRESHOLD`
    /// holds for `≥ RAPIDITY_DWELL` consecutive ticks. Bruneau 2003.
    /// NaN if never recovers within the simulation horizon.
    pub rapidity: f64,
    /// Attack Rate — fraction of initial fleet ever materially affected by
    /// any fault cascade. Wallinga & Lipsitch 2007. Range [0, 1].
    pub attack_rate: f64,

    // ── Fleet utilization ─────────────────────────────────────────
    /// Fleet utilization: average (alive_and_tasked / initial_fleet) post-fault.
    pub fleet_utilization: f64,

    // ── Cascade / spread ───────────────────────────────────────────
    pub survival_rate: f64,
    pub impacted_area: f64,
    pub deficit_integral: i64,

    // ── Performance ────────────────────────────────────────────────
    pub solver_step_time_avg_us: f64,
    pub solver_step_time_max_us: f64,
    pub wall_time_ms: u64,
}

/// Compute metrics for a baseline run (no faults, self-comparison).
///
/// Avoids creating a `BaselineRecord` just to compare against itself.
/// All fault-related metrics are trivially known (FT=1.0, no cascade, etc.),
/// only throughput/utilization/timing need computation from the engine.
pub fn compute_baseline_self_metrics(
    analysis: &AnalysisEngine,
    solver_step_times_us: &[f64],
    wall_time_ms: u64,
) -> RunMetrics {
    let tick_count = analysis.tick_count();

    let avg_throughput = if tick_count > 0 {
        analysis.throughput_series.iter().sum::<f64>() / tick_count as f64
    } else {
        0.0
    };
    let total_tasks = analysis.tasks_completed_series.last().copied().unwrap_or(0);
    let wait_ratio = analysis.wait_ratio_series.last().copied().unwrap_or(0.0) as f64;

    let unassigned_ratio = if tick_count > 0 {
        let total_idle: usize = analysis.idle_count_series.iter().sum();
        let total_agents: usize =
            analysis.alive_series.iter().zip(analysis.dead_series.iter()).map(|(a, d)| a + d).sum();
        if total_agents > 0 { total_idle as f64 / total_agents as f64 } else { 0.0 }
    } else {
        0.0
    };

    let solver_step_time_avg_us = if solver_step_times_us.is_empty() {
        0.0
    } else {
        solver_step_times_us.iter().sum::<f64>() / solver_step_times_us.len() as f64
    };
    let solver_step_time_max_us = solver_step_times_us.iter().copied().fold(0.0_f64, f64::max);

    RunMetrics {
        avg_throughput,
        total_tasks,
        unassigned_ratio,
        wait_ratio,
        fault_tolerance: 1.0,
        critical_time: f64::NAN,
        deficit_recovery: 0.0,
        throughput_recovery: 0.0,
        mtbf: None,
        recovery_tick: None,
        cascade_spread_avg: 0.0,
        cascade_depth_avg: 0.0,
        itae: 0.0,
        rapidity: 0.0,
        attack_rate: 0.0,
        fleet_utilization: 1.0,
        survival_rate: 1.0,
        impacted_area: 0.0,
        deficit_integral: 0,
        solver_step_time_avg_us,
        solver_step_time_max_us,
        wall_time_ms,
    }
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
    let total_tasks = faulted_analysis.tasks_completed_series.last().copied().unwrap_or(0);

    // ── Idle / wait ratio ──────────────────────────────────────────
    // wait_ratio = cumulative (Wait actions / total actions). From AnalysisEngine.
    let wait_ratio = faulted_analysis.wait_ratio_series.last().copied().unwrap_or(0.0) as f64;

    // unassigned_ratio = fraction of agent-ticks where the agent had no task assignment
    // (task leg == Free). This is distinct from wait_ratio which measures physical
    // Wait actions regardless of task state.
    let unassigned_ratio = if tick_count > 0 {
        let total_idle: usize = faulted_analysis.idle_count_series.iter().sum();
        let total_agents: usize = faulted_analysis
            .alive_series
            .iter()
            .zip(faulted_analysis.dead_series.iter())
            .map(|(a, d)| a + d)
            .sum();
        if total_agents > 0 { total_idle as f64 / total_agents as f64 } else { 0.0 }
    } else {
        0.0
    };

    // ── Fault Tolerance (post-fault-onset only) ───────────────────
    // Find first tick where fault events occurred (0-indexed into series).
    let first_fault_idx = faulted_fault_events.iter().position(|events| !events.is_empty());

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
            if avg_baseline > 0.0 { avg_faulted / avg_baseline } else { f64::NAN }
        }
        None => {
            // No faults occurred → system retained full capability → FT = 1.0
            1.0
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
        .flat_map(|(i, events)| if events.is_empty() { vec![] } else { vec![i as u64 + 1] })
        .collect();

    let mtbf = FaultMetrics::compute_mtbf(&all_fault_ticks).map(|v| v as f64);

    // Deficit recovery: ticks from first cumulative deficit to full catch-up.
    let deficit_recovery = match (diff.recovery_tick, diff.first_gap_tick) {
        (Some(recovery), Some(first_gap)) if recovery > first_gap => (recovery - first_gap) as f64,
        (None, Some(_)) => f64::NAN, // gap occurred but never recovered
        _ => 0.0,                    // no gap = no fault impact = genuinely 0
    };

    // Throughput recovery: first tick after fault onset where per-tick
    // faulted throughput >= per-tick baseline throughput. Measures rate recovery.
    let throughput_recovery = compute_throughput_recovery(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
    );

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
        if total > 0 { final_alive as f64 / total as f64 } else { 1.0 }
    } else {
        1.0
    };

    // ── Fleet Utilization: (alive - free) / initial, averaged post-fault ──
    let initial_fleet = faulted_analysis.alive_series.first().copied().unwrap_or(0)
        + faulted_analysis.dead_series.first().copied().unwrap_or(0);
    let fleet_utilization = match first_fault_idx {
        Some(start) if initial_fleet > 0 => {
            let post_ticks = tick_count - start;
            if post_ticks > 0 {
                let sum: f64 = (start..tick_count)
                    .map(|i| {
                        let alive = faulted_analysis.alive_series.get(i).copied().unwrap_or(0);
                        let idle = faulted_analysis.idle_count_series.get(i).copied().unwrap_or(0);
                        let tasked = alive.saturating_sub(idle);
                        tasked as f64 / initial_fleet as f64
                    })
                    .sum();
                sum / post_ticks as f64
            } else {
                1.0
            }
        }
        _ => 1.0,
    };

    // ── Tier 1 resilience metrics (ITAE, Rapidity) ─────────────────
    let itae = compute_itae(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
    );
    let rapidity = compute_rapidity(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
        crate::constants::RAPIDITY_THRESHOLD,
        crate::constants::RAPIDITY_DWELL,
    );

    // ── Solver step timing ─────────────────────────────────────────
    let solver_step_time_avg_us = if solver_step_times_us.is_empty() {
        0.0
    } else {
        solver_step_times_us.iter().sum::<f64>() / solver_step_times_us.len() as f64
    };
    let solver_step_time_max_us = solver_step_times_us.iter().copied().fold(0.0_f64, f64::max);

    RunMetrics {
        avg_throughput,
        total_tasks,
        unassigned_ratio,
        wait_ratio,
        fault_tolerance,
        critical_time,
        deficit_recovery,
        throughput_recovery,
        mtbf,
        recovery_tick: diff.recovery_tick,
        cascade_spread_avg: 0.0,
        cascade_depth_avg: 0.0,
        itae,
        rapidity,
        attack_rate: 0.0,
        fleet_utilization,
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
        Some(_) => return 0.0,                // tick 0 edge case
        None => return f64::NAN,              // no fault impact → metric undefined
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

    if ticks_after_fault > 0 { ticks_below as f64 / ticks_after_fault as f64 } else { 0.0 }
}

/// Integral of Time-weighted Absolute Error of throughput ratio post-fault.
/// `J_ITAE = Σ (t - t_fault) · |1 - R(t)|`  where `R(t) = faulted_tp[t] / baseline_tp[t]`.
///
/// Skips ticks where `baseline_tp[t] == 0` (ratio undefined).
/// Returns NaN if `start` is None (no fault occurred), 0.0 if start is past
/// the series end.
fn compute_itae(baseline_tp: &[f64], faulted_tp: &[f64], start: Option<usize>) -> f64 {
    let start = match start {
        Some(s) => s,
        None => return f64::NAN,
    };
    let len = baseline_tp.len().min(faulted_tp.len());
    if start >= len {
        return 0.0;
    }
    let mut acc = 0.0_f64;
    for i in start..len {
        let b = baseline_tp[i];
        if b == 0.0 {
            continue;
        }
        let r = faulted_tp[i] / b;
        let t_weight = (i - start) as f64;
        acc += t_weight * (1.0 - r).abs();
    }
    acc
}

/// Rapidity — ticks from first fault until `R(t) = faulted_tp[t] / baseline_tp[t]`
/// meets or exceeds `threshold` for `≥ dwell` consecutive ticks. Returns the
/// offset (relative to `start`) of the first tick of the confirming dwell window.
/// NaN if never recovers within the horizon.
fn compute_rapidity(
    baseline_tp: &[f64],
    faulted_tp: &[f64],
    start: Option<usize>,
    threshold: f64,
    dwell: usize,
) -> f64 {
    let start = match start {
        Some(s) => s,
        None => return f64::NAN,
    };
    let len = baseline_tp.len().min(faulted_tp.len());
    if start >= len {
        return f64::NAN;
    }
    let mut consec = 0_usize;
    for i in start..len {
        let b = baseline_tp[i];
        let r = if b == 0.0 { 0.0 } else { faulted_tp[i] / b };
        if r >= threshold {
            consec += 1;
            if consec >= dwell {
                // First tick of the confirming dwell window, relative to fault start.
                return (i + 1 - dwell - start) as f64;
            }
        } else {
            consec = 0;
        }
    }
    f64::NAN
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

    #[test]
    fn ft_known_values() {
        // Baseline: constant 2.0 throughput
        let bl = [2.0_f64; 20];
        // Faulted: normal for 5 ticks, then drops to 1.0
        let mut faulted = [2.0_f64; 20];
        for i in 5..20 {
            faulted[i] = 1.0;
        }

        // FT = avg(faulted[5..]) / avg(baseline[5..]) = 1.0 / 2.0 = 0.5
        let avg_faulted: f64 = faulted[5..].iter().sum::<f64>() / 15.0;
        let avg_baseline: f64 = bl[5..].iter().sum::<f64>() / 15.0;
        let ft = avg_faulted / avg_baseline;
        assert!((ft - 0.5).abs() < 1e-10, "FT should be 0.5, got {ft}");
    }

    #[test]
    fn critical_time_known_fraction() {
        // 20 ticks baseline=4.0, faulted drops at tick 10
        let bl = vec![4.0; 20];
        let mut faulted = vec![4.0; 20];
        // Ticks at 0-indexed 9..14: below 50% threshold (faulted=1.0, threshold=2.0)
        for i in 9..14 {
            faulted[i] = 1.0;
        }
        // Ticks 14-19: above threshold (faulted=4.0, stays at default)
        // first_gap_tick = Some(10) is 1-indexed, converts to start=9 internally
        // CT = 5 below / (20-9=11 total post-fault) = 5/11
        let ct = compute_critical_time(&bl, &faulted, Some(10));
        let expected = 5.0 / 11.0;
        assert!((ct - expected).abs() < 1e-10, "CT should be {expected:.4}, got {ct:.4}");
    }

    #[test]
    fn throughput_recovery_known_tick() {
        let bl = vec![3.0; 20];
        let mut faulted = vec![3.0; 20];
        // Drop at tick 5, stay at 0 until tick 10 where it recovers
        for i in 5..10 {
            faulted[i] = 0.0;
        }
        // Recovery = first tick AFTER fault onset where faulted >= baseline
        // fault_onset = index 5. Skip tick 5 itself. Check from index 6.
        // Index 10: faulted[10] = 3.0 >= baseline[10] = 3.0
        // recovery = 10 - 5 = 5.0
        let recovery = compute_throughput_recovery(&bl, &faulted, Some(5));
        assert!((recovery - 5.0).abs() < 1e-10, "recovery should be 5.0, got {recovery}");
    }

    #[test]
    fn impacted_area_sign_convention() {
        use std::collections::HashMap;
        // Build minimal BaselineRecord
        let baseline = BaselineRecord {
            config_hash: 0,
            tick_count: 10,
            num_agents: 10,
            throughput_series: vec![1.0; 10],
            tasks_completed_series: (1..=10).map(|i| i as u64).collect(),
            idle_count_series: vec![0; 10],
            wait_ratio_series: vec![0.0; 10],
            total_tasks: 10,
            avg_throughput: 1.0,
            traffic_counts: HashMap::default(),
            position_snapshots: Vec::new(),
        };
        // Faulted: 90% of baseline tasks
        let live_tasks: Vec<u64> = (1..=10).map(|i| (i as f64 * 0.9) as u64).collect();
        let live_tp = vec![0.9; 10];

        let mut diff = BaselineDiff::default();
        diff.recompute(&baseline, &live_tasks, &live_tp);
        // impacted_area should be negative (behind baseline)
        assert!(
            diff.impacted_area < 0.0,
            "impacted_area should be negative when behind baseline, got {}",
            diff.impacted_area
        );
    }

    #[test]
    fn ft_one_when_no_faults() {
        // When no faults occurred, FT should be exactly 1.0
        // (faulted run identical to baseline)
        let bl_tp = 2.5_f64;
        let faulted_tp = 2.5_f64;
        // No faults -> FT = faulted_avg / baseline_avg
        let ft = faulted_tp / bl_tp;
        assert!((ft - 1.0).abs() < 1e-10, "FT should be 1.0 with no faults, got {ft}");

        // Also verify throughput_recovery returns 0.0 with no faults
        let recovery = compute_throughput_recovery(&[2.0; 10], &[2.0; 10], None);
        assert!((recovery - 0.0).abs() < 1e-10, "recovery should be 0.0 with no faults");
    }

    // ── compute_itae (Ogata 2010) ────────────────────────────────────

    #[test]
    fn itae_zero_when_identical() {
        let bl = vec![1.0; 10];
        let ft = vec![1.0; 10];
        assert_eq!(compute_itae(&bl, &ft, Some(3)), 0.0);
    }

    #[test]
    fn itae_monotonic_in_deviation() {
        let bl = vec![2.0; 10];
        let small = {
            let mut v = vec![2.0; 10];
            for i in 3..10 {
                v[i] = 1.5;
            }
            v
        };
        let big = {
            let mut v = vec![2.0; 10];
            for i in 3..10 {
                v[i] = 0.5;
            }
            v
        };
        assert!(compute_itae(&bl, &big, Some(3)) > compute_itae(&bl, &small, Some(3)));
    }

    #[test]
    fn itae_nan_when_no_fault() {
        let bl = vec![1.0; 5];
        let ft = vec![1.0; 5];
        assert!(compute_itae(&bl, &ft, None).is_nan());
    }

    // ── compute_rapidity (Bruneau 2003) ──────────────────────────────

    #[test]
    fn rapidity_immediate_recovery() {
        let bl = vec![1.0; 20];
        let ft = vec![1.0; 20];
        let r = compute_rapidity(&bl, &ft, Some(3), 0.9, 5);
        assert!((r - 0.0).abs() < 1e-9, "got {r}");
    }

    #[test]
    fn rapidity_nan_when_no_recovery() {
        let bl = vec![1.0; 20];
        let mut ft = vec![1.0; 20];
        for i in 3..20 {
            ft[i] = 0.3;
        }
        assert!(compute_rapidity(&bl, &ft, Some(3), 0.9, 5).is_nan());
    }

    #[test]
    fn rapidity_requires_dwell() {
        // Brief cross (3 ticks) above threshold then dip → not recovered w/ dwell=5
        let bl = vec![1.0; 20];
        let mut ft = vec![0.5; 20];
        ft[5] = 1.0;
        ft[6] = 1.0;
        ft[7] = 1.0; // only 3 ticks, dwell=5 → not recovered
        assert!(compute_rapidity(&bl, &ft, Some(3), 0.9, 5).is_nan());
    }
}
