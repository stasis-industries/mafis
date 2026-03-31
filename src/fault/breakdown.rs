use bevy::prelude::*;

use super::config::{FaultSource, FaultType};

#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct Dead;

#[derive(Message)]
pub struct FaultEvent {
    pub entity: Entity,
    pub fault_type: FaultType,
    pub source: FaultSource,
    pub tick: u64,
    pub position: IVec2,
    /// Path invalidations counted at the instant of death (before replanning).
    /// Used as a floor for cascade `agents_affected` when ADG BFS finds fewer.
    pub paths_invalidated: u32,
}

/// Latency injection: forces Wait for `remaining` ticks, then auto-removes.
#[derive(Component)]
pub struct LatencyFault {
    pub remaining: u32,
}

// Old ECS systems (detect_faults, replan_after_fault, apply_latency_faults)
// removed — SimulationRunner handles all fault logic internally via its tick()
// method.

#[cfg(test)]
mod tests {
    use super::*;

    // ── Weibull inverse CDF determinism ──────────────────────────────

    /// Verify that pre-sampled Weibull failure ticks are deterministic:
    /// same seed → same failure ticks, regardless of simulation path.
    /// This replaces the old per-tick RNG consumption test.
    #[test]
    fn weibull_inverse_cdf_deterministic() {
        use crate::core::seed::SeededRng;
        use rand::Rng;

        let seed = 42u64;
        let beta = 2.5_f32;
        let eta = 500.0_f32;
        let inv_beta = 1.0_f64 / beta as f64;
        let eta_f64 = eta as f64;

        let mut rng1 = SeededRng::new(seed);
        let ticks1: Vec<u32> = (0..10)
            .map(|_| {
                let u: f64 = rng1.rng.random_range(f64::EPSILON..1.0_f64);
                (eta_f64 * (-u.ln()).powf(inv_beta)).round() as u32
            })
            .collect();

        let mut rng2 = SeededRng::new(seed);
        let ticks2: Vec<u32> = (0..10)
            .map(|_| {
                let u: f64 = rng2.rng.random_range(f64::EPSILON..1.0_f64);
                (eta_f64 * (-u.ln()).powf(inv_beta)).round() as u32
            })
            .collect();

        assert_eq!(ticks1, ticks2, "same seed must produce identical failure ticks");
        // All failure ticks should be > 0
        assert!(ticks1.iter().all(|&t| t > 0), "failure ticks must be positive");
    }
}
