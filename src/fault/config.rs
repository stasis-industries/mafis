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
/// **WearBased scenario** uses the Weibull failure model (Carlson & Murphy 2006;
/// CASUN 2023). At simulation init, each agent's failure time is pre-sampled via
/// inverse CDF: `t_fail = eta * (-ln(U))^(1/beta)`, U ~ Uniform(0,1).
/// Agents fail permanently when `operational_age >= t_fail`.
/// Literature basis: encoder/tire wear = 73.8% of AGV failures (INASE 2014);
/// degradation rate accelerates with accumulated use (beta > 1 = wear-out phase).
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
    /// Low = 2.0 (CASUN certified warehouse AGV); High = 3.5 (Carlson 2006 field robots).
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
}

impl FaultConfig {
    /// Validate that the configuration is internally consistent.
    /// Multi-fault lists may enable both weibull + intermittent — this is valid
    /// because they use separate RNG consumption patterns (Weibull pre-samples
    /// at init, intermittent samples per-tick from fault_rng).
    pub fn validate(&self) {
        // No assertions — all combinations are valid since multi-fault support.
    }
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
        }
    }
}
