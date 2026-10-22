//! Fault Tolerance (FT) — shared compute used by both the experiment runner
//! (`crate::experiment::metrics`) and the live observatory scorecard
//! (`crate::analysis::scorecard`).
//!
//! ```text
//! FT = mean(P_f[t_f..]) / mean(P_b[t_f..])
//! ```
//! Both means cover the post-fault window only, so pre-fault ticks (identical
//! in both runs) never dilute the fault effect. FT is undefined (`None`) when
//! the twin's post-fault mean is 0: there was no throughput to retain.

/// FT over the post-fault window: mean faulted throughput over mean twin
/// throughput. `None` when the twin's window is empty or its mean is not positive.
pub fn fault_tolerance(faulted_post: &[f64], baseline_post: &[f64]) -> Option<f64> {
    let baseline = mean(baseline_post)?;
    if baseline <= 0.0 {
        return None;
    }
    Some(mean(faulted_post).unwrap_or(0.0) / baseline)
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_throughput_is_one() {
        assert_eq!(fault_tolerance(&[1.0, 1.0], &[1.0, 1.0]), Some(1.0));
    }

    #[test]
    fn half_throughput_is_half() {
        assert_eq!(fault_tolerance(&[0.5, 0.5], &[1.0, 1.0]), Some(0.5));
    }

    #[test]
    fn braess_above_one() {
        assert_eq!(fault_tolerance(&[2.0], &[1.0]), Some(2.0));
    }

    #[test]
    fn window_of_one_tick() {
        assert_eq!(fault_tolerance(&[0.2], &[0.4]), Some(0.5));
    }

    #[test]
    fn means_not_sums_when_lengths_differ() {
        assert_eq!(fault_tolerance(&[1.0], &[1.0, 3.0]), Some(0.5));
    }

    #[test]
    fn empty_faulted_is_zero() {
        assert_eq!(fault_tolerance(&[], &[1.0]), Some(0.0));
    }

    #[test]
    fn zero_baseline_is_undefined() {
        assert_eq!(fault_tolerance(&[0.3], &[0.0]), None);
    }

    #[test]
    fn empty_baseline_is_undefined() {
        assert_eq!(fault_tolerance(&[0.3], &[]), None);
    }
}
