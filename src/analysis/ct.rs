//! Critical Time (CT) — shared compute used by both the experiment runner
//! (`crate::experiment::metrics`) and the live observatory scorecard
//! (`crate::analysis::scorecard`).
//!
//! Single source of truth for the CT formula. Prevents the experiment ↔
//! observatory drift that produced 0.180 vs 0.858 for the same config.
//!
//! Formula:
//! ```text
//! smoothed_baseline[i] = mean(baseline_tp[max(0, i+1-W)..=i])   // W = CT_BASELINE_WINDOW
//! threshold[i]         = smoothed_baseline[i] * CRITICAL_TIME_THRESHOLD
//! ticks_below          = count i in [first_fault_idx, end) where faulted_tp[i] < threshold[i]
//! CT                   = ticks_below / (end - first_fault_idx)
//! ```
//!
//! The rolling mean smooths the sparse per-tick task-completion signal at
//! low agent counts. Without smoothing, `baseline_tp[i]` is zero on most
//! ticks for ≤10-agent runs, the threshold collapses to zero, and
//! `faulted < 0` is never true → CT under-counts severely.

use crate::constants::{CRITICAL_TIME_THRESHOLD, CT_BASELINE_WINDOW};

/// Mean of `series[max(0, end_idx+1-window)..=end_idx]`.
/// Returns 0.0 if the series is empty or `end_idx` is past the end.
pub fn rolling_mean_at(series: &[f64], window: usize, end_idx: usize) -> f64 {
    if series.is_empty() || window == 0 {
        return 0.0;
    }
    let last = end_idx.min(series.len() - 1);
    let w = window.min(last + 1);
    let start = last + 1 - w;
    let sum: f64 = series[start..=last].iter().sum();
    sum / w as f64
}

/// Critical Time: fraction of post-fault ticks where faulted throughput
/// falls below `CRITICAL_TIME_THRESHOLD` of the rolling-mean baseline.
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

    let mut ticks_below = 0_usize;
    for i in start..len {
        let smoothed = rolling_mean_at(baseline_tp, CT_BASELINE_WINDOW, i);
        let threshold = smoothed * CRITICAL_TIME_THRESHOLD;
        if faulted_tp[i] < threshold {
            ticks_below += 1;
        }
    }

    let ticks_after_fault = len - start;
    ticks_below as f64 / ticks_after_fault as f64
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
        // Faulted == baseline → never below threshold.
        let bl = vec![1.0; 50];
        let ft = vec![1.0; 50];
        assert_eq!(critical_time(&bl, &ft, Some(20)), 0.0);
    }

    #[test]
    fn one_when_faulted_zero_post_fault() {
        // Baseline non-zero, faulted = 0 from fault onset → always below threshold.
        let bl = vec![2.0; 50];
        let mut ft = vec![2.0; 50];
        for slot in ft.iter_mut().skip(20) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(20));
        assert!((ct - 1.0).abs() < 1e-10, "expected 1.0, got {ct}");
    }

    #[test]
    fn half_when_half_below() {
        let bl = vec![2.0; 50];
        let mut ft = vec![2.0; 50];
        // 15 ticks below (zero), 15 ticks at baseline. start=20, total post-fault=30.
        for slot in ft.iter_mut().skip(20).take(15) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(20));
        assert!((ct - 0.5).abs() < 1e-10, "expected 0.5, got {ct}");
    }

    #[test]
    fn sparse_baseline_not_degenerate() {
        // Low-agent case: baseline has a single completion every 10 ticks
        // (avg ≈ 0.1/tick). Faulted produces no completions post-fault.
        // Without rolling-mean: baseline_tp[i]=0 most ticks → threshold=0
        //   → faulted (also 0) never strictly below → CT ≈ 0 (degenerate).
        // With rolling-mean W=10: smoothed baseline ≈ 0.1, threshold ≈ 0.05
        //   → faulted=0 falls below → CT should be near 1.0.
        let mut bl = vec![0.0; 100];
        for i in (9..100).step_by(10) {
            bl[i] = 1.0;
        }
        let mut ft = bl.clone();
        for slot in ft.iter_mut().skip(50) {
            *slot = 0.0;
        }
        let ct = critical_time(&bl, &ft, Some(50));
        assert!(
            ct > 0.5,
            "rolling-mean smoothing should keep CT non-degenerate for sparse signals, got {ct}"
        );
    }

    #[test]
    fn rolling_mean_basic() {
        let s = vec![0.0, 0.0, 10.0, 0.0, 0.0];
        // window=3, end_idx=2 → mean of [0.0, 0.0, 10.0] = 10/3
        let m = rolling_mean_at(&s, 3, 2);
        assert!((m - 10.0 / 3.0).abs() < 1e-10);
        // window=3, end_idx=4 → mean of [10.0, 0.0, 0.0] = 10/3
        let m = rolling_mean_at(&s, 3, 4);
        assert!((m - 10.0 / 3.0).abs() < 1e-10);
        // window=3, end_idx=0 → only 1 sample available → mean = 0.0
        let m = rolling_mean_at(&s, 3, 0);
        assert!((m - 0.0).abs() < 1e-10);
    }

    #[test]
    fn rolling_mean_clamps_end_idx() {
        let s = vec![1.0, 2.0, 3.0];
        // end_idx past end → clamps to last index.
        let m = rolling_mean_at(&s, 2, 99);
        assert!((m - 2.5).abs() < 1e-10);
    }

    #[test]
    fn rolling_mean_empty_series() {
        let s: Vec<f64> = vec![];
        assert_eq!(rolling_mean_at(&s, 5, 0), 0.0);
    }

    #[test]
    fn start_past_end_returns_zero() {
        let bl = vec![1.0; 10];
        let ft = vec![1.0; 10];
        assert_eq!(critical_time(&bl, &ft, Some(50)), 0.0);
    }
}
