use bevy::prelude::*;

// ---------------------------------------------------------------------------
// SimulationPhase
// ---------------------------------------------------------------------------

/// With the dual-twin model (headless baseline + fault simulation), there is
/// no warmup phase. The simulation is always in the "running" phase.
/// We keep FaultInjection as the single active phase for scorecard/metrics
/// that gate on `is_fault_injection()`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimulationPhase {
    #[default]
    Running,
}

impl SimulationPhase {
    pub fn is_warmup(self) -> bool {
        false
    }

    pub fn is_fault_injection(self) -> bool {
        true
    }

    pub fn label(self) -> &'static str {
        match self {
            SimulationPhase::Running => "running",
        }
    }
}

// ---------------------------------------------------------------------------
// ResilienceBaseline (kept for backward compat with scorecard/metrics)
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Clone)]
pub struct ResilienceBaseline {
    pub baseline_throughput: f64,
    pub baseline_wait_ratio: f32,
    pub warmup_complete: bool,
}

impl Default for ResilienceBaseline {
    fn default() -> Self {
        Self { baseline_throughput: 0.0, baseline_wait_ratio: 0.0, warmup_complete: false }
    }
}

impl ResilienceBaseline {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct PhasePlugin;

impl Plugin for PhasePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SimulationPhase>().init_resource::<ResilienceBaseline>();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── ResilienceBaseline lifecycle ──────────────────────────────────

    #[test]
    fn resilience_baseline_reset_clears_all_fields() {
        let mut baseline = ResilienceBaseline {
            baseline_throughput: 1.5,
            baseline_wait_ratio: 0.3,
            warmup_complete: true,
        };
        baseline.reset();
        assert!(!baseline.warmup_complete);
        assert_eq!(baseline.baseline_throughput, 0.0);
        assert_eq!(baseline.baseline_wait_ratio, 0.0);
    }
}
