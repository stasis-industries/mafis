use bevy::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum FaultType {
    Overheat,
    Breakdown,
    Latency,
}

impl FaultType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overheat => "Overheat",
            Self::Breakdown => "Breakdown",
            Self::Latency => "Latency",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum FaultSource {
    Automatic,
    Manual,
    Scheduled,
}

impl FaultSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Manual => "Manual",
            Self::Scheduled => "Scheduled",
        }
    }
}

/// Fault configuration for the continuous automatic fault model.
///
/// **WearBased scenario** uses the Weibull failure model calibrated from
/// Carlson & Murphy 2005 (field robot reliability study). At simulation init,
/// each agent's failure time is pre-sampled via inverse CDF:
/// `t_fail = eta * (-ln(U))^(1/beta)`, U ~ Uniform(0,1).
/// Agents fail permanently when `operational_age >= t_fail`.
/// Literature basis: mechanical wear (encoder/tire/gear) dominates field robot
/// failures; degradation rate accelerates with accumulated use (beta > 1 =
/// wear-out phase of the Weibull bathtub curve).
///
/// **IntermittentFault scenario** models temporary unavailability via exponential
/// inter-arrival times: each agent independently samples its next fault from
/// Exp(1/mtbf_ticks). Faults inject latency (not permanent death) -- agent
/// recovers after `intermittent_recovery_ticks`.
#[derive(Resource, Debug, Clone, Serialize)]
pub struct FaultConfig {
    pub enabled: bool,

    // -- Weibull wear model (WearBased scenario) --
    /// Whether to run the Weibull wear detection each tick.
    pub weibull_enabled: bool,
    /// Shape parameter beta (beta > 1 = wear-out, increasing failure rate).
    /// Low = 2.0 (well-maintained warehouse AGV); High = 3.5 (Carlson & Murphy 2005 field robots).
    pub weibull_beta: f32,
    /// Scale parameter eta -- characteristic life in operational movement-ticks.
    /// Accelerated time scale; calibrated to produce failures within experiment window.
    pub weibull_eta: f32,

    // -- Intermittent fault model (IntermittentFault scenario) --
    /// Whether to run intermittent fault injection each tick.
    pub intermittent_enabled: bool,
    /// Mean ticks between failures per agent (exponential inter-arrival).
    pub intermittent_mtbf_ticks: u64,
    /// How many ticks an agent is unavailable per intermittent fault event.
    pub intermittent_recovery_ticks: u32,
    /// Earliest tick at which intermittent faults can fire. Before this tick,
    /// `check_intermittent_faults` is a no-op (no sampling, no events).
    /// First fire becomes `start_tick + Exp(MTBF)` giving a deterministic
    /// warm-up window across seeds while preserving memoryless inter-arrivals.
    /// Default 0 = backward-compatible (sample from tick 0 as before).
    pub intermittent_start_tick: u64,
}

impl Default for FaultConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            weibull_enabled: false,
            weibull_beta: 2.5,
            weibull_eta: 500.0,
            intermittent_enabled: false,
            intermittent_mtbf_ticks: 100,
            intermittent_recovery_ticks: 20,
            intermittent_start_tick: 0,
        }
    }
}
