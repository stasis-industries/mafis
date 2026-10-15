//! Windowed throughput ratio `R_w(t)` and Critical Time (CT) — shared compute
//! used by both the experiment runner (`crate::experiment::metrics`) and the
//! live observatory scorecard (`crate::analysis::scorecard`).
//!
//! Single source of truth for the windowed ratio behind CT and Rapidity (TWTE
//! uses the cumulative post-fault ratio, see `experiment::metrics::compute_twte`).
//! Prevents the experiment ↔ observatory drift that produced 0.180 vs 0.858
//! for the same config.
//!
//! Per-tick task completions are sparse integers (most ticks are 0), so a
//! per-tick ratio `P_f(t) / P_b(t)` is mostly 0/0 or 0/1 noise. Instead:
//! ```text
//! R_w(t) = Σ_{s=a..=t} P_f(s) / Σ_{s=a..=t} P_b(s),   a = max(t_f, t + 1 − W),  W = RATIO_WINDOW
//! ```
//! Only post-fault ticks enter the window (it grows from 1 tick at `t_f` to W),
//! so pre-fault ticks — identical in both runs — never dilute the fault effect.
//! Faulted and baseline are summed over the same window. `R_w(t)` is undefined
//! (`None`) when the baseline window sum is 0.
//!
//! ```text
//! CT = |{ t ∈ [t_f, T) : R_w(t) < CRITICAL_TIME_THRESHOLD }| / (T − t_f)
//! ```
//! Undefined ticks are not critical: the baseline itself delivered nothing in
//! the window, so there is no throughput to fall below.

use crate::constants::{CRITICAL_TIME_THRESHOLD, RATIO_WINDOW};

/// `R_w(t)` for series index `t ≥ start` (see module docs). `None` when the
/// baseline window sum is 0 or `t` is outside either series.
pub fn ratio_at(baseline_tp: &[f64], faulted_tp: &[f64], start: usize, t: usize) -> Option<f64> {
    if t < start || t >= baseline_tp.len().min(faulted_tp.len()) {
        return None;
    }
    let a = start.max((t + 1).saturating_sub(RATIO_WINDOW));
    let b: f64 = baseline_tp[a..=t].iter().sum();
    if b <= 0.0 {
        return None;
    }
    let f: f64 = faulted_tp[a..=t].iter().sum();
    Some(f / b)
}

/// `R_w(t)` for every post-fault tick: element `k` is `R_w(start + k)`.
pub fn windowed_ratio(baseline_tp: &[f64], faulted_tp: &[f64], start: usize) -> Vec<Option<f64>> {
    let len = baseline_tp.len().min(faulted_tp.len());
    (start..len).map(|t| ratio_at(baseline_tp, faulted_tp, start, t)).collect()
}

/// Critical Time: fraction of post-fault ticks where `R_w(t)` falls below
/// `CRITICAL_TIME_THRESHOLD`.
///
/// `first_fault_idx` is the 0-indexed series position of the first tick with
/// fault events. Returns `NaN` if `None` (metric undefined when no fault).
pub fn critical_time(
    baseline_tp: &[f64],
    faulted_tp: &[f64],
    first_fault_idx: Option<usize>,
) -> f64 {
    let start = match first_fault_idx {
        Some(s) => s,
        None => return f64::NAN,
    };

    let len = baseline_tp.len().min(faulted_tp.len());
    if start >= len {
        return 0.0;
    }

    let ticks_below = windowed_ratio(baseline_tp, faulted_tp, start)
        .iter()
        .filter(|r| r.is_some_and(|r| r < CRITICAL_TIME_THRESHOLD))
        .count();
    ticks_below as f64 / (len - start) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_when_no_fault() {
        let bl = vec![1.0; 50];
        let ft = vec![1.0; 50];
        assert!(critical_time(&bl, &ft, None).is_nan());
    }

    #[test]
    fn zero_when_identical_post_fault() {
        // Faulted == baseline → R_w = 1 → never below threshold.
        let bl = vec![1.0; 50];
        let ft = vec![1.0; 50];
        assert_eq!(critical_time(&bl, &ft, Some(20)), 0.0);
    }

    #[test]
    fn one_when_faulted_zero_post_fault() {
        // Baseline non-zero, faulted = 0 from fault onset → R_w = 0 every tick.
        let bl = vec![2.0; 50];
        let mut ft = vec![2.0; 50];
        for slot in ft.iter_mut().skip(20) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(20));
        assert!((ct - 1.0).abs() < 1e-10, "expected 1.0, got {ct}");
    }

    #[test]
    fn outage_then_recovery_counts_window_lag() {
        // Baseline 2.0/tick; faulted 0 for 15 post-fault ticks (k = 0..14), then 2.0.
        // k ≤ 19 (window still growing, k+1 ticks): R_w = (k−14)⁺/(k+1) < 0.5 → critical.
        // k ≥ 20 (full 20-tick window): R_w = (k−14)/20 < 0.5 ↔ k ≤ 23 → critical.
        // Critical: k = 0..=23 → 24 of 30 post-fault ticks.
        let bl = vec![2.0; 50];
        let mut ft = vec![2.0; 50];
        for slot in ft.iter_mut().skip(20).take(15) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(20));
        assert!((ct - 24.0 / 30.0).abs() < 1e-10, "expected 0.8, got {ct}");
    }

    #[test]
    fn single_zero_tick_is_not_critical() {
        // One post-fault tick with zero completions while the baseline completed one:
        // a per-tick ratio would call it critical (0 < 0.5); R_w at that tick is 5/6.
        let bl = vec![1.0; 60];
        let mut ft = vec![1.0; 60];
        ft[25] = 0.0;
        assert_eq!(critical_time(&bl, &ft, Some(20)), 0.0);
    }

    #[test]
    fn sparse_baseline_not_degenerate() {
        // Baseline completes one task every 10 ticks (indices 9, 19, ...).
        // Faulted produces nothing from tick 50. Window ticks 50..=58 hold no
        // baseline completion (undefined, not critical); from 59 on R_w = 0.
        let mut bl = vec![0.0; 100];
        for i in (9..100).step_by(10) {
            bl[i] = 1.0;
        }
        let mut ft = bl.clone();
        for slot in ft.iter_mut().skip(50) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(50));
        assert!((ct - 41.0 / 50.0).abs() < 1e-10, "expected 0.82, got {ct}");
    }

    #[test]
    fn ratio_at_grows_from_fault_onset() {
        let bl = [1.0, 1.0, 1.0, 1.0];
        let ft = [9.0, 0.0, 1.0, 1.0];
        // Pre-fault tick 0 (9 vs 1) must not enter the window.
        assert_eq!(ratio_at(&bl, &ft, 1, 1), Some(0.0));
        assert_eq!(ratio_at(&bl, &ft, 1, 2), Some(0.5));
        assert_eq!(ratio_at(&bl, &ft, 1, 3), Some(2.0 / 3.0));
        assert_eq!(ratio_at(&bl, &ft, 1, 0), None); // before fault onset
        assert_eq!(ratio_at(&bl, &ft, 1, 9), None); // past the end
    }

    #[test]
    fn ratio_at_undefined_on_empty_baseline_window() {
        let bl = [0.0, 0.0, 1.0];
        let ft = [1.0, 1.0, 1.0];
        assert_eq!(ratio_at(&bl, &ft, 0, 1), None);
        assert_eq!(ratio_at(&bl, &ft, 0, 2), Some(3.0));
    }

    #[test]
    fn ratio_window_caps_at_ratio_window() {
        // Faulted is 0 for the first 5 post-fault ticks, then matches the baseline.
        // Once the window has slid past them (k ≥ W + 4), R_w is exactly 1.
        let start = 10;
        let len = start + RATIO_WINDOW + 5;
        let bl = vec![1.0; len];
        let mut ft = vec![1.0; len];
        for slot in ft.iter_mut().skip(start).take(5) {
            *slot = 0.0;
        }
        let last = start + RATIO_WINDOW + 4;
        assert_eq!(ratio_at(&bl, &ft, start, last), Some(1.0));
        assert!(ratio_at(&bl, &ft, start, last - 1).unwrap() < 1.0);
    }

    #[test]
    fn start_past_end_returns_zero() {
        let bl = vec![1.0; 10];
        let ft = vec![1.0; 10];
        assert_eq!(critical_time(&bl, &ft, Some(50)), 0.0);
    }
}
