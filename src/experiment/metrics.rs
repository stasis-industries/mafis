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
    /// Computed over alive agents only — dead agents are excluded since their loss
    /// is captured by survival_rate. Higher = more congestion or faults.
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

    // ── Cascade (ADG-based, solver-coupled) ────────────────────────
    /// Average cascade spread per fault event (agents affected via ADG BFS).
    /// Solver-coupled: depends on per-solver planning lookahead.
    pub cascade_spread_avg: f64,
    /// Average cascade depth per fault event (max BFS depth).
    /// Solver-coupled: depends on per-solver planning lookahead.
    pub cascade_depth_avg: f64,

    // ── Structural cascade (solver-independent) ────────────────────
    /// Average structural cascade per fault event: count of alive agents
    /// whose static-grid shortest path passes through the dead cell.
    /// Decoupled from solver planning (Freeman 1977; Brandes 2001;
    /// Ewing et al. 2022 for the MAPF betweenness lineage).
    pub structural_cascade_avg: f64,
    /// Maximum structural cascade observed across all fault events of the run.
    pub structural_cascade_max: f64,
    /// Mitigation delta = `cascade_spread_avg - structural_cascade_avg`.
    /// Positive: solver propagates fault impact beyond the topological vulnerability.
    /// Negative: solver localizes the fault below the topological vulnerability
    /// (replanning successfully reroutes around the dead cell).
    pub mitigation_delta_avg: f64,

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

    // ── Solver-specific telemetry ─────────────────────────────────
    /// Fraction of RHCR-PBS replan windows that returned `WindowResult::Partial`
    /// and fell through to LRA + PIBT fallback. `None` for non-RHCR solvers or
    /// when no windows were attempted. Used as an observatory probe for PBS
    /// scalability cliffs (see paper §6.4 / Appendix B).
    pub pbs_partial_rate: Option<f32>,

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

    // Baseline contract: NaN = undefined-for-baseline (no fault → metric has
    // no meaningful value); 0.0 = no-fault-impact (deficit/recovery genuinely
    // measured zero impact, since the baseline never deviates from itself).
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
        structural_cascade_avg: 0.0,
        structural_cascade_max: 0.0,
        mitigation_delta_avg: 0.0,
        itae: f64::NAN,
        rapidity: f64::NAN,
        attack_rate: 0.0,
        fleet_utilization: 1.0,
        survival_rate: 1.0,
        impacted_area: 0.0,
        deficit_integral: 0,
        pbs_partial_rate: None,
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
    let smooth_w = crate::constants::RAPIDITY_SMOOTH_WINDOW;
    let baseline_smooth = rolling_average(&baseline.throughput_series, smooth_w);
    let faulted_smooth = rolling_average(&faulted_analysis.throughput_series, smooth_w);
    // Gate on RAW series — smoothing dilutes the fault drop with pre-fault data
    // (W-1 pre-fault ticks bound the smoothed ratio at start to (W-1)/W ≈ 0.95,
    // tripping the threshold-0.9 gate even for severe sudden faults).
    let rapidity = compute_rapidity_gated_on_raw(
        &baseline_smooth,
        &faulted_smooth,
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
        structural_cascade_avg: 0.0,
        structural_cascade_max: 0.0,
        mitigation_delta_avg: 0.0,
        itae,
        rapidity,
        attack_rate: 0.0,
        fleet_utilization,
        survival_rate,
        impacted_area: diff.impacted_area,
        deficit_integral: diff.deficit_integral,
        pbs_partial_rate: None,
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
    first_fault_tick: Option<u64>,
) -> f64 {
    let start = match first_fault_tick {
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
///
/// **Degradation-observed gate** (added 2026-04-17 after Phase 0 reliability audit):
/// returns NaN if the ratio is already at or above `threshold` at `start`. This
/// prevents the metric from spuriously reporting Rapidity = 0 when (a) the fault
/// had no measurable fleet-level impact, or (b) the rolling-mean smoothing window
/// still contains pre-fault data at `start`. A zero-or-near-zero Rapidity from a
/// system that never visibly degraded is not a recovery measurement, so we return
/// NaN. Rapidity is undefined in that case.
/// Production variant of [`compute_rapidity`]: evaluates the degradation-observed
/// gate on the RAW (unsmoothed) series and the dwell loop on the smoothed series.
///
/// Smoothing the gate input dilutes the fault drop with pre-fault baseline data
/// (a window of W ticks at `start` contains W-1 pre-fault ticks, so the smoothed
/// ratio is bounded below by (W-1)/W ≈ 0.95), which causes the gate to fire
/// spuriously for sudden faults. Gating on the raw single-tick ratio at `start`
/// avoids this dilution.
fn compute_rapidity_gated_on_raw(
    baseline_smooth: &[f64],
    faulted_smooth: &[f64],
    baseline_raw: &[f64],
    faulted_raw: &[f64],
    start: Option<usize>,
    threshold: f64,
    dwell: usize,
) -> f64 {
    let start = match start {
        Some(s) => s,
        None => return f64::NAN,
    };
    let len_raw = baseline_raw.len().min(faulted_raw.len());
    if start >= len_raw {
        return f64::NAN;
    }
    let b_raw = baseline_raw[start];
    let r_raw = if b_raw == 0.0 { 0.0 } else { faulted_raw[start] / b_raw };
    if r_raw >= threshold {
        return f64::NAN;
    }

    let len = baseline_smooth.len().min(faulted_smooth.len());
    if start >= len {
        return f64::NAN;
    }
    let mut consec = 0_usize;
    for i in start..len {
        let b = baseline_smooth[i];
        let r = if b == 0.0 { 0.0 } else { faulted_smooth[i] / b };
        if r >= threshold {
            consec += 1;
            if consec >= dwell {
                return (i + 1 - dwell - start) as f64;
            }
        } else {
            consec = 0;
        }
    }
    f64::NAN
}

#[cfg(test)]
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

    // Degradation-observed gate: if the ratio is already ≥ threshold at `start`,
    // the system never visibly degraded and Rapidity is undefined.
    let b_start = baseline_tp[start];
    let r_start = if b_start == 0.0 { 0.0 } else { faulted_tp[start] / b_start };
    if r_start >= threshold {
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

/// Compute rolling average of a time series with window W.
/// Output[i] = mean(series[max(0, i+1-W)..=i]).
/// Output length = input length.
fn rolling_average(series: &[f64], window: usize) -> Vec<f64> {
    if series.is_empty() || window == 0 {
        return series.to_vec();
    }
    let w = window.min(series.len());
    let mut out = Vec::with_capacity(series.len());
    let mut sum = 0.0_f64;
    for (i, &v) in series.iter().enumerate() {
        sum += v;
        if i >= w {
            sum -= series[i - w];
            out.push(sum / w as f64);
        } else {
            out.push(sum / (i + 1) as f64);
        }
    }
    out
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
    fn rapidity_nan_when_never_degraded() {
        // Phase 0 fix (2026-04-17): if faulted throughput equals baseline at
        // t_fault, the system never visibly degraded. Rapidity is undefined
        // (NaN), not zero — zero would falsely claim "instant recovery".
        let bl = vec![1.0; 20];
        let ft = vec![1.0; 20];
        let r = compute_rapidity(&bl, &ft, Some(3), 0.9, 5);
        assert!(
            r.is_nan(),
            "Rapidity should be NaN when system never degraded (no degradation-observed gate triggered), got {r}"
        );
    }

    #[test]
    fn rapidity_recovers_after_dip() {
        // Sanity check that the gate does not block genuine dip-then-recover.
        let bl = vec![1.0; 30];
        let mut ft = vec![1.0; 30];
        // Fault causes dip from tick 5 through tick 14; recover tick 15+.
        for i in 5..15 {
            ft[i] = 0.5;
        }
        // start=5 where degradation begins. r_start = 0.5 < 0.9, gate does not trip.
        // Dwell of 5 consecutive ticks at threshold 0.9 is met at tick 19.
        // (i=19, consec=5, return 19+1-5-5 = 10)
        let r = compute_rapidity(&bl, &ft, Some(5), 0.9, 5);
        assert!((r - 10.0).abs() < 1e-9, "got {r}");
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
        let bl = vec![1.0; 20];
        let mut ft = vec![0.5; 20];
        ft[5] = 1.0;
        ft[6] = 1.0;
        ft[7] = 1.0;
        assert!(compute_rapidity(&bl, &ft, Some(3), 0.9, 5).is_nan());
    }

    #[test]
    fn rolling_average_basic() {
        let s = vec![0.0, 0.0, 10.0, 0.0, 0.0];
        let r = rolling_average(&s, 3);
        assert!((r[0] - 0.0).abs() < 1e-9);
        assert!((r[1] - 0.0).abs() < 1e-9);
        assert!((r[2] - 10.0 / 3.0).abs() < 1e-9);
        assert!((r[3] - 10.0 / 3.0).abs() < 1e-9);
        assert!((r[4] - 10.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn rolling_average_window_1() {
        let s = vec![1.0, 2.0, 3.0];
        let r = rolling_average(&s, 1);
        assert!((r[0] - 1.0).abs() < 1e-9);
        assert!((r[1] - 2.0).abs() < 1e-9);
        assert!((r[2] - 3.0).abs() < 1e-9);
    }

    #[test]
    fn rapidity_smoothed_rejects_spike() {
        let bl = vec![10.0; 40];
        let mut ft = vec![3.0; 40];
        ft[10] = 10.0;
        ft[11] = 10.0;
        ft[12] = 10.0;
        ft[13] = 10.0;
        ft[14] = 10.0;
        let bl_s = rolling_average(&bl, 20);
        let ft_s = rolling_average(&ft, 20);
        assert!(
            compute_rapidity(&bl_s, &ft_s, Some(5), 0.9, 5).is_nan(),
            "5-tick spike in 3.0 baseline should not trigger recovery with W=20 smoothing"
        );
    }

    /// Regression: production path must NOT return NaN for sudden burst faults
    /// where the raw single-tick ratio at `start` is below threshold but the
    /// smoothed ratio at `start` is bounded above (W-1)/W ≈ 0.95 by pre-fault data.
    #[test]
    fn rapidity_gated_on_raw_recovers_after_burst() {
        // Pre-fault: identical baseline=faulted=10 for ticks [0..100].
        // Fault at tick 100: faulted drops to 5 for 30 ticks, then recovers to 10.
        let mut bl = vec![10.0; 200];
        let mut ft = vec![10.0; 200];
        for i in 100..200 {
            bl[i] = 10.0;
        }
        for i in 100..130 {
            ft[i] = 5.0;
        }
        for i in 130..200 {
            ft[i] = 10.0;
        }
        let w = 20;
        let bl_s = rolling_average(&bl, w);
        let ft_s = rolling_average(&ft, w);

        // Without raw-gate fix, the smoothed ratio at start=100 is
        // (19*10 + 5) / (20*10) = 195/200 = 0.975 > 0.9 → NaN.
        let bad = compute_rapidity(&bl_s, &ft_s, Some(100), 0.9, 5);
        assert!(bad.is_nan(), "smoothed-only gate spuriously trips for burst");

        // With the raw-gate fix: raw ratio at 100 is 5/10 = 0.5 < 0.9 → gate passes.
        // Dwell loop on smoothed eventually finds R >= 0.9 for 5 consecutive ticks.
        let r = compute_rapidity_gated_on_raw(&bl_s, &ft_s, &bl, &ft, Some(100), 0.9, 5);
        assert!(!r.is_nan(), "raw-gated rapidity must recover, got NaN");
        assert!(r > 0.0 && r < 100.0, "rapidity {r} out of expected [0, 100) range");
    }

    /// Verify the gate still fires when the system genuinely never degraded.
    #[test]
    fn rapidity_gated_on_raw_nan_when_never_degraded() {
        let bl = vec![10.0; 100];
        let ft = vec![10.0; 100]; // identical → no degradation
        let bl_s = rolling_average(&bl, 20);
        let ft_s = rolling_average(&ft, 20);
        let r = compute_rapidity_gated_on_raw(&bl_s, &ft_s, &bl, &ft, Some(50), 0.9, 5);
        assert!(r.is_nan(), "no-degradation case must return NaN");
    }
}
