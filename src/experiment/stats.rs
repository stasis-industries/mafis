//! Statistical summary and confidence intervals for experiment results.
//!
//! Pure math — no dependencies on simulation code. Testable standalone.

/// Summary statistics for a single metric across multiple seeds.
#[derive(Debug, Clone)]
pub struct StatSummary {
    pub n: usize,
    pub mean: f64,
    pub std: f64,
    pub ci95_lo: f64,
    pub ci95_hi: f64,
    pub min: f64,
    pub max: f64,
}

impl Default for StatSummary {
    fn default() -> Self {
        Self {
            n: 0,
            mean: f64::NAN,
            std: f64::NAN,
            ci95_lo: f64::NAN,
            ci95_hi: f64::NAN,
            min: f64::NAN,
            max: f64::NAN,
        }
    }
}

/// Compute summary statistics from a slice of values.
/// NaN values are filtered out before computation (they represent undefined metrics).
/// Returns `None` if no finite values remain after filtering.
pub fn compute_stat_summary(values: &[f64]) -> Option<StatSummary> {
    // Filter out NaN values — these represent undefined metrics from edge-case runs
    let values: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
    let n = values.len();
    if n == 0 {
        return None;
    }

    let mean = values.iter().sum::<f64>() / n as f64;

    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    let std = if n > 1 {
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        variance.sqrt()
    } else {
        0.0
    };

    let (ci95_lo, ci95_hi) = if n > 1 {
        let t = t_critical_95(n);
        let margin = t * std / (n as f64).sqrt();
        (mean - margin, mean + margin)
    } else {
        (mean, mean)
    };

    Some(StatSummary {
        n,
        mean,
        std,
        ci95_lo,
        ci95_hi,
        min,
        max,
    })
}

/// Two-tailed t-critical value for 95% confidence (α=0.05).
/// Uses a lookup table for df 1..30, then the z-value 1.96 for df > 30.
pub fn t_critical_95(n: usize) -> f64 {
    let df = n.saturating_sub(1);
    // Table: t_{0.025, df} for two-tailed 95% CI
    const TABLE: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, // df 1-5
        2.447, 2.365, 2.306, 2.262, 2.228, // df 6-10
        2.201, 2.179, 2.160, 2.145, 2.131, // df 11-15
        2.120, 2.110, 2.101, 2.093, 2.086, // df 16-20
        2.080, 2.074, 2.069, 2.064, 2.060, // df 21-25
        2.056, 2.052, 2.048, 2.045, 2.042, // df 26-30
    ];

    if df == 0 {
        return f64::INFINITY;
    }
    if df <= 30 {
        TABLE[df - 1]
    } else {
        1.96 // z-value approximation for large df
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_none() {
        assert!(compute_stat_summary(&[]).is_none());
    }

    #[test]
    fn single_value() {
        let s = compute_stat_summary(&[5.0]).unwrap();
        assert_eq!(s.n, 1);
        assert_eq!(s.mean, 5.0);
        assert_eq!(s.std, 0.0);
        assert_eq!(s.min, 5.0);
        assert_eq!(s.max, 5.0);
        assert_eq!(s.ci95_lo, 5.0);
        assert_eq!(s.ci95_hi, 5.0);
    }

    #[test]
    fn two_values() {
        let s = compute_stat_summary(&[2.0, 4.0]).unwrap();
        assert_eq!(s.n, 2);
        assert!((s.mean - 3.0).abs() < 1e-10);
        assert!((s.std - std::f64::consts::SQRT_2).abs() < 1e-10);
        assert_eq!(s.min, 2.0);
        assert_eq!(s.max, 4.0);
        // CI should be wider than ±std for n=2
        assert!(s.ci95_lo < s.mean);
        assert!(s.ci95_hi > s.mean);
    }

    #[test]
    fn identical_values() {
        let s = compute_stat_summary(&[7.0, 7.0, 7.0]).unwrap();
        assert_eq!(s.std, 0.0);
        assert_eq!(s.ci95_lo, 7.0);
        assert_eq!(s.ci95_hi, 7.0);
    }

    #[test]
    fn known_std() {
        // values: 2, 4, 4, 4, 5, 5, 7, 9
        // mean = 5, sample std ≈ 2.138
        let vals = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let s = compute_stat_summary(&vals).unwrap();
        assert_eq!(s.n, 8);
        assert!((s.mean - 5.0).abs() < 1e-10);
        assert!((s.std - 2.138).abs() < 0.01);
    }

    #[test]
    fn t_critical_boundary() {
        assert_eq!(t_critical_95(1), f64::INFINITY); // df=0
        assert!((t_critical_95(2) - 12.706).abs() < 1e-3); // df=1
        assert!((t_critical_95(31) - 2.042).abs() < 1e-3); // df=30
        assert!((t_critical_95(100) - 1.96).abs() < 1e-3); // df>30
    }

    #[test]
    fn nan_values_filtered() {
        // NaN values should be excluded from statistics
        let s = compute_stat_summary(&[1.0, f64::NAN, 3.0, f64::NAN]).unwrap();
        assert_eq!(s.n, 2);
        assert!((s.mean - 2.0).abs() < 1e-10);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 3.0);
    }

    #[test]
    fn all_nan_returns_none() {
        assert!(compute_stat_summary(&[f64::NAN, f64::NAN]).is_none());
    }

    #[test]
    fn ci_narrows_with_more_samples() {
        let few = compute_stat_summary(&[1.0, 2.0, 3.0]).unwrap();
        let many = compute_stat_summary(&[1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0]).unwrap();
        let few_width = few.ci95_hi - few.ci95_lo;
        let many_width = many.ci95_hi - many.ci95_lo;
        assert!(many_width < few_width, "CI should narrow with more samples");
    }

    #[test]
    fn ci95_matches_reference() {
        // Reference: [2, 4, 4, 4, 5, 5, 7, 9], n=8, mean=5.0
        // Sample std = sqrt(sum((xi-5)^2)/7) = sqrt((9+1+1+1+0+0+4+16)/7) = sqrt(32/7) ~ 2.1381
        // t(df=7, 0.025) = 2.365 (from table)
        // margin = 2.365 * 2.1381 / sqrt(8) = 2.365 * 0.7559 = 1.7867
        // CI = [5.0 - 1.787, 5.0 + 1.787] = [3.213, 6.787]
        let vals = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let s = compute_stat_summary(&vals).unwrap();
        assert!(
            (s.ci95_lo - 3.21).abs() < 0.02,
            "ci95_lo should be ~3.21, got {:.3}",
            s.ci95_lo
        );
        assert!(
            (s.ci95_hi - 6.79).abs() < 0.02,
            "ci95_hi should be ~6.79, got {:.3}",
            s.ci95_hi
        );
    }

    #[test]
    fn nan_filtering_sample_size() {
        // 5 values with 2 NaNs -> effective n=3
        let vals = [1.0, f64::NAN, 3.0, f64::NAN, 5.0];
        let s = compute_stat_summary(&vals).unwrap();
        assert_eq!(s.n, 3, "effective sample size should be 3");
        assert!(
            (s.mean - 3.0).abs() < 1e-10,
            "mean of [1,3,5] should be 3.0"
        );
        // std of [1,3,5] = sqrt(((1-3)^2+(3-3)^2+(5-3)^2)/2) = sqrt(8/2) = 2.0
        assert!((s.std - 2.0).abs() < 1e-10, "std should be 2.0, got {}", s.std);
        // CI with t(df=2) = 4.303
        let margin = 4.303 * 2.0 / (3.0_f64).sqrt();
        let expected_lo = 3.0 - margin;
        let expected_hi = 3.0 + margin;
        assert!(
            (s.ci95_lo - expected_lo).abs() < 0.01,
            "ci95_lo mismatch"
        );
        assert!(
            (s.ci95_hi - expected_hi).abs() < 0.01,
            "ci95_hi mismatch"
        );
    }
}
