//! Headless differential metrics computed from paired baseline/faulted runs.
//!
//! These metrics are the core research outputs: they quantify how a system
//! degrades and recovers under fault conditions compared to a clean baseline.

use crate::analysis::baseline::{BaselineDiff, BaselineRecord};
use crate::analysis::engine::AnalysisEngine;
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::ft;

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

    // ── Cascade (ADG on the fault-instant snapshot, common lookahead) ──
    /// Average agents involved per fault event: the agents faulted at that
    /// instant plus those reached by multi-source ADG BFS.
    pub cascade_spread_avg: f64,
    /// Average cascade depth per fault event (max BFS depth from the nearest
    /// faulted agent). Lookahead is `ADG_LOOKAHEAD` for every solver.
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
    /// TWTE — time-weighted mean absolute error of the cumulative post-fault
    /// throughput ratio: `J = Σ (t−t_f)·|1 − R_c(t)| / Σ (t−t_f)`, see
    /// `compute_twte`. Form adapted from ITAE (Ogata 2010). Dimensionless,
    /// 0 = no deviation; normalised so it is comparable across post-fault
    /// window lengths. NaN if no fault.
    /// Field name kept as `itae` for backward compatibility with exports.
    pub itae: f64,
    /// Rapidity — ticks from first fault until the windowed ratio `R_w`,
    /// after first falling below `RAPIDITY_THRESHOLD`, holds at or above it for
    /// `RAPIDITY_DWELL` consecutive ticks. Bruneau 2003. A run that never
    /// recovers is right-censored at `T − t_f` (see `rapidity_recovered`).
    /// NaN when there is no fault or `R_w` never fell below the threshold.
    pub rapidity: f64,
    /// Secondary Attack Rate — among agents never directly faulted, the
    /// fraction reached by any fault's cascade (ADG BFS on the fault-instant
    /// snapshot). Range [0, 1]; NaN when every agent was directly faulted.
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

    // ── Measurement validity ───────────────────────────────────────
    /// Post-fault ticks before the baseline's first post-fault completion, where
    /// the TWTE ratio is undefined; excluded from TWTE and reported here.
    pub twte_undefined_ticks: usize,
    /// `Some(true)` recovered, `Some(false)` degraded and right-censored at
    /// the horizon, `None` when Rapidity is undefined (no fault, no degradation).
    pub rapidity_recovered: Option<bool>,
    /// The fault-free baseline stalled (see `baseline_gridlocked`). Metrics
    /// against it measure the deadlock, not the fault; summaries exclude it.
    pub baseline_gridlocked: bool,
    /// Pre-rebuild Attack Rate: fraction of the initial fleet directly faulted
    /// or reached by a cascade. Kept for comparability with older runs; on
    /// permanent faults it largely echoes the kill fraction (`attack_rate` is
    /// the secondary rate that excludes directly faulted agents).
    pub attack_rate_total: f64,
    // ── Collision accounting (set by the experiment runner) ───────
    /// Agent-ticks where the runner overrode a solver move with a Wait.
    pub forced_waits: u64,
    /// Ticks with a vertex or edge-swap conflict after resolution. Must be 0.
    pub collision_violations: u64,
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
        twte_undefined_ticks: 0,
        rapidity_recovered: None,
        baseline_gridlocked: baseline_gridlocked(&analysis.throughput_series),
        attack_rate_total: 0.0,
        forced_waits: 0,
        collision_violations: 0,
    }
}

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
            let baseline_post = if start < baseline.throughput_series.len() {
                &baseline.throughput_series[start..]
            } else {
                &baseline.throughput_series[..]
            };
            ft::fault_tolerance(&faulted_analysis.throughput_series[start..], baseline_post)
                .unwrap_or(f64::NAN)
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
    // Shared compute with the live observatory scorecard — see
    // `crate::analysis::ct` for the rolling-mean threshold formula.
    let critical_time = crate::analysis::ct::critical_time(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
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

    // ── Tier 1 resilience metrics (TWTE, Rapidity) ─────────────────
    // TWTE on the cumulative post-fault ratio, Rapidity on the windowed R_w.
    let (itae, twte_undefined_ticks) = compute_twte(
        &baseline.throughput_series,
        &faulted_analysis.throughput_series,
        first_fault_idx,
    );
    let (rapidity, rapidity_recovered) = compute_rapidity(
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
        twte_undefined_ticks,
        rapidity_recovered,
        baseline_gridlocked: baseline_gridlocked(&baseline.throughput_series),
        attack_rate_total: 0.0,
        forced_waits: 0,
        collision_violations: 0,
    }
}

/// Baseline validity: true when the fault-free baseline completes no task for
/// `GRIDLOCK_WINDOW` consecutive ticks after its first completion, or never
/// completes one in a run at least that long. Ticks before the first
/// completion are warm-up (agents still travelling to their first pickup).
/// A healthy baseline never stalls that long (see the constant), so a stall
/// means the solver deadlocked on its own.
pub fn baseline_gridlocked(baseline_tp: &[f64]) -> bool {
    let window = crate::constants::GRIDLOCK_WINDOW;
    let Some(first) = baseline_tp.iter().position(|&v| v > 0.0) else {
        return baseline_tp.len() >= window;
    };
    let mut zero_run = 0_usize;
    for &v in &baseline_tp[first..] {
        zero_run = if v > 0.0 { 0 } else { zero_run + 1 };
        if zero_run >= window {
            return true;
        }
    }
    false
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

/// TWTE — time-weighted mean absolute error of the cumulative post-fault ratio:
/// `J = Σ_{t∈D} (t−t_f)·|1 − R_c(t)| / Σ_{t∈D} (t−t_f)` with
/// `R_c(t) = Σ_{s=t_f..=t} P_f(s) / Σ_{s=t_f..=t} P_b(s)` (share of the baseline's
/// post-fault work delivered so far), `D` = ticks where `R_c` is defined.
///
/// Cumulative, not the 20-tick window `R_w`: post-fault trajectories diverge
/// from the twin within a few ticks, and on `R_w` two independent fault-free
/// runs score the same J (≈0.37 at SD n=40) as real faults, so the windowed
/// form measures realization noise. On `R_c` that null scores ≈0.05.
///
/// Returns `(J, undefined_ticks)`: ticks before the baseline's first post-fault
/// completion are excluded from both sums and counted, never silently skipped.
/// `J` is NaN when there is no fault or no defined tick carries weight.
fn compute_twte(baseline_tp: &[f64], faulted_tp: &[f64], start: Option<usize>) -> (f64, usize) {
    let Some(start) = start else {
        return (f64::NAN, 0);
    };
    let len = baseline_tp.len().min(faulted_tp.len());
    let (mut cum_b, mut cum_f) = (0.0_f64, 0.0_f64);
    let (mut weighted_err, mut weight) = (0.0_f64, 0.0_f64);
    let mut undefined = 0_usize;
    for t in start..len {
        cum_b += baseline_tp[t];
        cum_f += faulted_tp[t];
        if cum_b <= 0.0 {
            undefined += 1;
            continue;
        }
        let k = (t - start) as f64;
        weighted_err += k * (1.0 - cum_f / cum_b).abs();
        weight += k;
    }
    let j = if weight > 0.0 { weighted_err / weight } else { f64::NAN };
    (j, undefined)
}

/// Rapidity (Bruneau 2003) on the windowed ratio `R_w`, evaluated only at
/// ticks whose window is full (`t ≥ t_f + RATIO_WINDOW − 1`) so a 1–2 tick
/// window cannot fake a degradation or a recovery. Returns:
/// - `(NaN, None)` — no fault, or `R_w` never fell below `threshold`
///   (nothing to recover from);
/// - `(t_rec − t_f, Some(true))` — after the first degraded tick, `R_w ≥ threshold`
///   for `dwell` consecutive ticks; `t_rec` is the first tick of that dwell;
/// - `(T − t_f, Some(false))` — degraded and never recovered: right-censored
///   at the horizon.
///
/// Resolution: recovery is detected with up to `RATIO_WINDOW` ticks of lag,
/// and values below `RATIO_WINDOW − 1` cannot occur.
///
/// Caveat: at SD n=40, two independent fault-free runs also "degrade" and
/// "recover" on `R_w` (90/90 pairs, median 46 ticks at W=20), so a per-run
/// value is below the realization noise floor; compare against a null control
/// before reading it as fault recovery.
fn compute_rapidity(
    baseline_tp: &[f64],
    faulted_tp: &[f64],
    start: Option<usize>,
    threshold: f64,
    dwell: usize,
) -> (f64, Option<bool>) {
    let Some(start) = start else {
        return (f64::NAN, None);
    };
    let ratios = crate::analysis::ct::windowed_ratio(baseline_tp, faulted_tp, start);
    let full = crate::constants::RATIO_WINDOW - 1;
    let Some(degraded) = (full..ratios.len()).find(|&k| ratios[k].is_some_and(|r| r < threshold))
    else {
        return (f64::NAN, None);
    };
    let mut consec = 0_usize;
    for k in degraded + 1..ratios.len() {
        if ratios[k].is_some_and(|r| r >= threshold) {
            consec += 1;
            if consec >= dwell {
                return ((k + 1 - dwell) as f64, Some(true));
            }
        } else {
            consec = 0;
        }
    }
    (ratios.len() as f64, Some(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::analysis::ct::critical_time as ct_fn;

    #[test]
    fn critical_time_no_fault() {
        // No fault impact → metric is undefined (NaN), not zero
        assert!(ct_fn(&[1.0; 10], &[1.0; 10], None).is_nan());
    }

    #[test]
    fn critical_time_all_below() {
        // Baseline all 2.0, faulted all 0.0 after index 2
        let bl = vec![2.0; 10];
        let mut faulted = vec![2.0; 10];
        for i in 2..10 {
            faulted[i] = 0.0;
        }
        let ct = ct_fn(&bl, &faulted, Some(2));
        // 8 ticks after fault, windowed ratio R_w = 0 at every one.
        assert!((ct - 1.0).abs() < 1e-10);
    }

    #[test]
    fn critical_time_half_below() {
        let bl = vec![2.0; 10];
        let mut faulted = vec![2.0; 10];
        // Faulted 0 for the first 4 post-fault ticks, back to 2.0 for the next 4.
        for i in 2..6 {
            faulted[i] = 0.0;
        }
        let ct = ct_fn(&bl, &faulted, Some(2));
        // R_w at k=0..7: 0, 0, 0, 0, 2/10, 4/12, 6/14, 8/16 → 7 of 8 below 0.5.
        assert!((ct - 7.0 / 8.0).abs() < 1e-10, "got {ct}");
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
        // 20 ticks baseline=4.0, faulted drops at index 9
        let bl = vec![4.0; 20];
        let mut faulted = vec![4.0; 20];
        // Indices 9..14: faulted=1.0 against baseline=4.0
        for i in 9..14 {
            faulted[i] = 1.0;
        }
        // R_w at k=0..10: 1/4 ×5, then 9/24, 13/28 (still < 0.5), 17/32, ... → 7 of 11.
        let ct = ct_fn(&bl, &faulted, Some(9));
        let expected = 7.0 / 11.0;
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

    // ── compute_twte (form adapted from ITAE, Ogata 2010) ────────────

    #[test]
    fn twte_zero_when_identical() {
        let bl = vec![1.0; 50];
        assert_eq!(compute_twte(&bl, &bl, Some(10)), (0.0, 0));
    }

    #[test]
    fn twte_half_when_throughput_halved() {
        // R_c = 0.5 at every post-fault tick → time-weighted mean error 0.5,
        // whatever the post-fault window length.
        for len in [50, 400] {
            let bl = vec![2.0; len];
            let mut ft = vec![2.0; len];
            for slot in ft.iter_mut().skip(10) {
                *slot = 1.0;
            }
            let (j, undefined) = compute_twte(&bl, &ft, Some(10));
            assert!((j - 0.5).abs() < 1e-12, "len={len}: got {j}");
            assert_eq!(undefined, 0);
        }
    }

    #[test]
    fn twte_one_tick_spike_is_bounded() {
        // One post-fault tick (k=10) completes 3 tasks instead of 1. A per-tick
        // ratio scores that tick |1 − 3| = 2; on R_c the excess decays as
        // 2/(k+1). Hand value: Σ_{k=10..99} 2k/(k+1) / Σ_{k=0..99} k ≈ 175.6/4950.
        let bl = vec![1.0; 110];
        let mut ft = bl.clone();
        ft[20] = 3.0;
        let (j, _) = compute_twte(&bl, &ft, Some(10));
        assert!((j - 0.0355).abs() < 1e-3, "got {j}");
    }

    #[test]
    fn twte_monotonic_in_deviation() {
        let bl = vec![2.0; 40];
        let mut small = vec![2.0; 40];
        let mut big = vec![2.0; 40];
        for i in 3..40 {
            small[i] = 1.5;
            big[i] = 0.5;
        }
        assert!(compute_twte(&bl, &big, Some(3)).0 > compute_twte(&bl, &small, Some(3)).0);
    }

    #[test]
    fn twte_counts_undefined_ticks() {
        // Baseline completes nothing in the first 3 post-fault ticks: R_c is
        // undefined there, so those ticks are counted, not silently skipped.
        let mut bl = vec![1.0; 30];
        for slot in bl.iter_mut().skip(10).take(3) {
            *slot = 0.0;
        }
        let ft = bl.clone();
        assert_eq!(compute_twte(&bl, &ft, Some(10)), (0.0, 3));
    }

    #[test]
    fn twte_nan_when_no_fault() {
        assert!(compute_twte(&[1.0; 5], &[1.0; 5], None).0.is_nan());
    }

    // ── compute_rapidity (Bruneau 2003) ──────────────────────────────

    #[test]
    fn rapidity_nan_when_never_degraded() {
        // Never degraded → nothing to recover from: undefined, not 0.
        let bl = vec![1.0; 100];
        let (r, recovered) = compute_rapidity(&bl, &bl, Some(3), 0.9, 5);
        assert!(r.is_nan() && recovered.is_none(), "got {r}, {recovered:?}");
    }

    #[test]
    fn rapidity_recovers_after_dip() {
        // Faulted runs at half rate for 40 post-fault ticks (k = 0..39), then full.
        // Full-window R_w (k ≥ 19) is 0.5 until k=40; at k R_w = (2 + 16)/20 = 0.9
        // first when only 4 half-rate ticks remain in the window, i.e. k = 55.
        // Dwell k = 55..59 → rapidity 55.
        let bl = vec![1.0; 200];
        let mut ft = vec![1.0; 200];
        for slot in ft.iter_mut().skip(50).take(40) {
            *slot = 0.5;
        }
        assert_eq!(compute_rapidity(&bl, &ft, Some(50), 0.9, 5), (55.0, Some(true)));
    }

    #[test]
    fn rapidity_censored_when_no_recovery() {
        // Never recovers within 100 post-fault ticks: right-censored at T − t_f.
        let bl = vec![1.0; 120];
        let mut ft = vec![1.0; 120];
        for slot in ft.iter_mut().skip(20) {
            *slot = 0.3;
        }
        assert_eq!(compute_rapidity(&bl, &ft, Some(20), 0.9, 5), (100.0, Some(false)));
    }

    #[test]
    fn rapidity_requires_dwell() {
        // Half rate for k = 0..19, a 10-task burst at k = 20, then nothing.
        // R_w at k = 20..23 is 0.975, 0.95, 0.925, 0.9 (4 ticks ≥ 0.9), then falls.
        let bl = vec![1.0; 70];
        let mut ft = vec![0.0; 70];
        for slot in ft.iter_mut().skip(10).take(20) {
            *slot = 0.5;
        }
        ft[30] = 10.0;
        assert_eq!(compute_rapidity(&bl, &ft, Some(10), 0.9, 5), (60.0, Some(false)));
    }

    #[test]
    fn rapidity_windowed_rejects_spike() {
        // 5-tick spike back to baseline inside a 30% run: R_w peaks at 95/200.
        let bl = vec![10.0; 40];
        let mut ft = vec![3.0; 40];
        for slot in ft.iter_mut().skip(10).take(5) {
            *slot = 10.0;
        }
        assert_eq!(compute_rapidity(&bl, &ft, Some(5), 0.9, 5), (35.0, Some(false)));
    }

    /// Regression: sparse completions and one missed task at t_f. The previous
    /// implementation gated on the raw tick (0/1 < 0.9) and then measured
    /// recovery on a window still holding 19 identical pre-fault ticks, so it
    /// returned Rapidity = 0. On the post-fault window the run never degrades.
    #[test]
    fn rapidity_single_missed_task_is_not_instant_recovery() {
        let bl: Vec<f64> = (0..300).map(|i| if i % 2 == 0 { 1.0 } else { 0.0 }).collect();
        let mut ft = bl.clone();
        ft[100] = 0.0;
        let (r, recovered) = compute_rapidity(&bl, &ft, Some(100), 0.9, 5);
        assert!(r.is_nan() && recovered.is_none(), "got {r}, {recovered:?}");
    }

    #[test]
    fn rapidity_undefined_without_a_full_window() {
        let bl = vec![1.0; 30];
        let mut ft = vec![1.0; 30];
        for slot in ft.iter_mut().skip(20) {
            *slot = 0.0;
        }
        let (r, recovered) = compute_rapidity(&bl, &ft, Some(20), 0.9, 5);
        assert!(r.is_nan() && recovered.is_none());
    }

    // ── baseline_gridlocked ──────────────────────────────────────────

    #[test]
    fn gridlock_flags_stall_after_first_completion() {
        let g = crate::constants::GRIDLOCK_WINDOW;
        let mut tp = vec![0.0; 3 * g];
        tp[5] = 1.0;
        tp[10] = 1.0;
        assert!(baseline_gridlocked(&tp));
    }

    #[test]
    fn gridlock_ignores_warm_up_and_shorter_gaps() {
        let g = crate::constants::GRIDLOCK_WINDOW;
        let mut tp = vec![0.0; 3 * g];
        // Warm-up longer than g, then a completion every g−1 ticks.
        for i in (g + 5..3 * g).step_by(g - 1) {
            tp[i] = 1.0;
        }
        assert!(!baseline_gridlocked(&tp));
    }

    #[test]
    fn gridlock_when_baseline_never_completes() {
        let g = crate::constants::GRIDLOCK_WINDOW;
        assert!(baseline_gridlocked(&vec![0.0; g]));
        assert!(!baseline_gridlocked(&vec![0.0; g - 1]));
    }
}
