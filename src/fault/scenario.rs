//! Fault Scenarios — 3 named presets with per-scenario parameters.
//!
//! Researchers pick a scenario and configure its specific parameters.
//! The system translates these into `FaultConfig` settings and a `FaultSchedule`
//! of timed events. A headless baseline twin runs alongside for comparison.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::config::FaultConfig;

// ---------------------------------------------------------------------------
// Scenario types (3 only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FaultScenarioType {
    /// Kill X% of robots at tick T.
    #[default]
    BurstFailure,
    /// Busiest robots fail permanently via Weibull wear model (continuous).
    WearBased,
    /// Latency on all robots in highest-traffic zone for N ticks.
    ZoneOutage,
    /// Each robot independently fails temporarily via exponential inter-arrival times.
    IntermittentFault,
}

impl FaultScenarioType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::BurstFailure    => "Burst Failure",
            Self::WearBased       => "Wear-Based",
            Self::ZoneOutage      => "Zone Outage",
            Self::IntermittentFault => "Intermittent Fault",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::BurstFailure    => "burst_failure",
            Self::WearBased       => "wear_based",
            Self::ZoneOutage      => "zone_outage",
            Self::IntermittentFault => "intermittent_fault",
        }
    }
}

impl FromStr for FaultScenarioType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "burst_failure"      => Self::BurstFailure,
            "wear_based"         => Self::WearBased,
            "zone_outage"        => Self::ZoneOutage,
            "intermittent_fault" => Self::IntermittentFault,
            _ => Self::BurstFailure,
        })
    }
}

// ---------------------------------------------------------------------------
// Wear intensity presets -- mapped to Weibull (beta, eta) parameters
// ---------------------------------------------------------------------------

/// Wear intensity preset for the WearBased scenario.
///
/// Maps to Weibull (beta, eta) parameters calibrated to produce realistic failure
/// distributions within a 500-tick experiment window:
///
/// | Level  | beta | eta | MTTF  | ~% dead at tick 500 | Literature basis                |
/// |--------|------|-----|-------|---------------------|---------------------------------|
/// | Low    | 2.0  | 900 | ~800t |  ~27%               | CASUN AGV, well-maintained      |
/// | Medium | 2.5  | 500 | ~445t |  ~63%               | Canadian survey 500-1,000 h     |
/// | High   | 3.5  | 150 | ~137t |  ~90%               | Carlson & Murphy 2006: MTBF=24h |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum WearHeatRate {
    Low,
    #[default]
    Medium,
    High,
}

impl WearHeatRate {
    pub fn id(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Weibull (beta, eta) parameters for this intensity level.
    /// beta = shape (wear-out acceleration); eta = scale (characteristic life in movement-ticks).
    pub fn weibull_params(&self) -> (f32, f32) {
        match self {
            Self::Low    => (2.0, 900.0),  // ~27% dead by tick 500
            Self::Medium => (2.5, 500.0),  // ~63% dead by tick 500
            Self::High   => (3.5, 150.0),  // ~90% dead by tick 500
        }
    }

    /// Ordering proxy (for tests only -- not used in fault pipeline).
    pub fn multiplier(&self) -> f32 {
        match self {
            Self::Low => 1.0,
            Self::Medium => 2.0,
            Self::High => 3.5,
        }
    }
}

impl FromStr for WearHeatRate {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "low" => Self::Low,
            "high" => Self::High,
            _ => Self::Medium,
        })
    }
}

// ---------------------------------------------------------------------------
// FaultScenario — the user's chosen configuration
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Clone, Serialize)]
pub struct FaultScenario {
    pub enabled: bool,
    pub scenario_type: FaultScenarioType,

    // -- Burst Failure params --
    pub burst_kill_percent: f32,
    pub burst_at_tick: u64,

    // -- Wear-Based params --
    pub wear_heat_rate: WearHeatRate,
    /// Overheat threshold kept for UI compatibility -- unused in Weibull model.
    pub wear_threshold: f32,

    // -- Zone Outage params --
    pub zone_at_tick: u64,
    pub zone_latency_duration: u32,

    // -- Intermittent Fault params --
    /// Mean ticks between failures per agent (exponential inter-arrival).
    pub intermittent_mtbf_ticks: u64,
    /// Ticks an agent is unavailable per fault event (latency injection).
    pub intermittent_recovery_ticks: u32,

    // -- Custom Weibull override --
    /// When set, overrides `wear_heat_rate` preset with custom (beta, eta).
    #[serde(skip)]
    pub custom_weibull: Option<(f32, f32)>,
}

impl Default for FaultScenario {
    fn default() -> Self {
        Self {
            enabled: false,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            wear_heat_rate: WearHeatRate::Medium,
            wear_threshold: 80.0,
            zone_at_tick: 100,
            zone_latency_duration: 50,
            intermittent_mtbf_ticks: 80,
            intermittent_recovery_ticks: 15,
            custom_weibull: None,
        }
    }
}

impl FaultScenario {
    /// Whether this scenario requires running a baseline for comparison.
    pub fn needs_baseline(&self) -> bool {
        self.enabled
    }

    pub fn to_fault_config(&self) -> FaultConfig {
        if !self.enabled {
            return FaultConfig {
                enabled: false,
                ..Default::default()
            };
        }

        match self.scenario_type {
            FaultScenarioType::WearBased => {
                let (beta, eta) = if let Some((b, e)) = self.custom_weibull {
                    (b, e)
                } else {
                    self.wear_heat_rate.weibull_params()
                };
                FaultConfig {
                    enabled: true,
                    weibull_enabled: true,
                    weibull_beta: beta,
                    weibull_eta: eta,
                    intermittent_enabled: false,
                    ..Default::default()
                }
            }
            FaultScenarioType::IntermittentFault => FaultConfig {
                enabled: true,
                weibull_enabled: false,
                intermittent_enabled: true,
                intermittent_mtbf_ticks: self.intermittent_mtbf_ticks,
                intermittent_recovery_ticks: self.intermittent_recovery_ticks,
                ..Default::default()
            },
            // Burst + Zone use scheduled events -- automatic fault model disabled
            _ => FaultConfig {
                enabled: false,
                ..Default::default()
            },
        }
    }

    /// Generate the fault schedule for this scenario.
    pub fn generate_schedule(&self, total_ticks: u64, num_agents: usize) -> FaultSchedule {
        if !self.enabled {
            return FaultSchedule::default();
        }

        let mut events = Vec::new();

        match self.scenario_type {
            FaultScenarioType::BurstFailure => {
                let at_tick = self.burst_at_tick.min(total_ticks);
                let count = ((num_agents as f32 * self.burst_kill_percent / 100.0)
                    .round() as usize)
                    .max(1)
                    .min(num_agents);
                events.push(ScheduledEvent {
                    tick: at_tick,
                    action: ScheduledAction::KillRandomAgents(count),
                    fired: false,
                });
            }
            FaultScenarioType::WearBased => {
                // No scheduled events -- Weibull model runs continuously via FaultConfig
            }
            FaultScenarioType::IntermittentFault => {
                // No scheduled events -- continuous per-agent model via FaultConfig
            }
            FaultScenarioType::ZoneOutage => {
                let at_tick = self.zone_at_tick.min(total_ticks);
                events.push(ScheduledEvent {
                    tick: at_tick,
                    action: ScheduledAction::ZoneLatency {
                        duration: self.zone_latency_duration,
                    },
                    fired: false,
                });
            }
        }

        FaultSchedule {
            events,
            initialized: true,
        }
    }
}

// ---------------------------------------------------------------------------
// FaultList — multi-fault configuration (UI-facing)
// ---------------------------------------------------------------------------

/// A single fault item in the multi-fault list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultItem {
    pub fault_type: FaultScenarioType,
    // Burst
    pub burst_kill_percent: f32,
    pub burst_at_tick: u64,
    // Wear
    pub wear_heat_rate: WearHeatRate,
    pub custom_weibull: Option<(f32, f32)>,
    // Zone
    pub zone_at_tick: u64,
    pub zone_latency_duration: u32,
    // Intermittent
    pub intermittent_mtbf_ticks: u64,
    pub intermittent_recovery_ticks: u32,
}

impl Default for FaultItem {
    fn default() -> Self {
        Self {
            fault_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            wear_heat_rate: WearHeatRate::Medium,
            custom_weibull: None,
            zone_at_tick: 100,
            zone_latency_duration: 50,
            intermittent_mtbf_ticks: 80,
            intermittent_recovery_ticks: 15,
        }
    }
}

/// Multi-fault configuration — replaces single FaultScenario for interactive runs.
///
/// Rules:
/// - Burst: multiple allowed, same-tick → sum percentages (cap 100%)
/// - Zone outage: multiple allowed, same-tick → max duration
/// - Wear-based: max ONE (continuous Weibull process)
/// - Intermittent: max ONE (continuous per-agent recurring)
/// - All types can be combined in one run
#[derive(Resource, Debug, Clone, Default, Serialize)]
pub struct FaultList {
    pub items: Vec<FaultItem>,
}

impl FaultList {
    /// Whether any faults are configured (needs baseline comparison).
    pub fn is_active(&self) -> bool {
        !self.items.is_empty()
    }

    /// Compile the fault list into a FaultConfig + FaultSchedule for the runner.
    ///
    /// Merges same-tick events: bursts sum percentages, zones take max duration.
    pub fn compile(&self, total_ticks: u64, num_agents: usize) -> (FaultConfig, FaultSchedule) {
        if self.items.is_empty() {
            return (FaultConfig { enabled: false, ..Default::default() }, FaultSchedule::default());
        }

        let mut fault_config = FaultConfig { enabled: false, ..Default::default() };
        let mut events: Vec<ScheduledEvent> = Vec::new();

        // --- Continuous models (at most one each) ---
        if let Some(wear) = self.items.iter().find(|i| i.fault_type == FaultScenarioType::WearBased) {
            let (beta, eta) = wear.custom_weibull.unwrap_or_else(|| wear.wear_heat_rate.weibull_params());
            fault_config.enabled = true;
            fault_config.weibull_enabled = true;
            fault_config.weibull_beta = beta;
            fault_config.weibull_eta = eta;
        }

        if let Some(inter) = self.items.iter().find(|i| i.fault_type == FaultScenarioType::IntermittentFault) {
            fault_config.enabled = true;
            fault_config.intermittent_enabled = true;
            fault_config.intermittent_mtbf_ticks = inter.intermittent_mtbf_ticks;
            fault_config.intermittent_recovery_ticks = inter.intermittent_recovery_ticks;
        }

        // --- Scheduled events: burst (sum same-tick) ---
        let mut burst_by_tick: std::collections::BTreeMap<u64, f32> = std::collections::BTreeMap::new();
        for item in self.items.iter().filter(|i| i.fault_type == FaultScenarioType::BurstFailure) {
            let tick = item.burst_at_tick.min(total_ticks);
            *burst_by_tick.entry(tick).or_insert(0.0) += item.burst_kill_percent;
        }
        for (tick, pct) in &burst_by_tick {
            let clamped = pct.min(100.0);
            let count = ((num_agents as f32 * clamped / 100.0).round() as usize).max(1).min(num_agents);
            events.push(ScheduledEvent { tick: *tick, action: ScheduledAction::KillRandomAgents(count), fired: false });
        }

        // --- Scheduled events: zone outage (max duration same-tick) ---
        let mut zone_by_tick: std::collections::BTreeMap<u64, u32> = std::collections::BTreeMap::new();
        for item in self.items.iter().filter(|i| i.fault_type == FaultScenarioType::ZoneOutage) {
            let tick = item.zone_at_tick.min(total_ticks);
            let dur = zone_by_tick.entry(tick).or_insert(0);
            *dur = (*dur).max(item.zone_latency_duration);
        }
        for (tick, duration) in &zone_by_tick {
            events.push(ScheduledEvent { tick: *tick, action: ScheduledAction::ZoneLatency { duration: *duration }, fired: false });
        }

        // Sort by tick for deterministic execution
        events.sort_by_key(|e| e.tick);

        let has_scheduled = !events.is_empty();
        let schedule = FaultSchedule { events, initialized: has_scheduled || fault_config.enabled };

        // Enable config if any scheduled events exist (runner checks this for schedule execution)
        if has_scheduled {
            fault_config.enabled = true;
        }

        (fault_config, schedule)
    }
}

// ---------------------------------------------------------------------------
// FaultSchedule — timed fault events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ScheduledEvent {
    pub tick: u64,
    pub action: ScheduledAction,
    pub fired: bool,
}

#[derive(Debug, Clone)]
pub enum ScheduledAction {
    /// Kill N random alive agents.
    KillRandomAgents(usize),
    /// Inject latency on all agents in highest-traffic zone for N ticks.
    ZoneLatency { duration: u32 },
}

#[derive(Resource, Debug, Clone, Default)]
pub struct FaultSchedule {
    pub events: Vec<ScheduledEvent>,
    pub initialized: bool,
}

impl FaultSchedule {
    pub fn clear(&mut self) {
        self.events.clear();
        self.initialized = false;
    }

    /// Reset `fired` flag on scheduled events after `tick` so they re-fire on resume.
    ///
    /// The snapshot at `tick` captures state AFTER that tick's processing, so events
    /// AT `tick` have already fired. Only events AFTER `tick` need to be un-fired.
    pub fn un_fire_after_tick(&mut self, tick: u64) {
        for event in &mut self.events {
            if event.tick > tick {
                event.fired = false;
            }
        }
    }

    /// Remove all scheduled events at or after `tick`. Used by delete-fault.
    pub fn remove_events_at_or_after(&mut self, tick: u64) {
        self.events.retain(|e| e.tick < tick);
    }
}

// Old execute_fault_schedule ECS system removed —
// SimulationRunner executes the schedule internally via its tick() method.

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_scenario_disabled() {
        let s = FaultScenario::default();
        assert!(!s.enabled);
        assert!(!s.needs_baseline());
    }

    #[test]
    fn enabled_scenario_needs_baseline() {
        let mut s = FaultScenario::default();
        s.enabled = true;
        assert!(s.needs_baseline());
    }

    #[test]
    fn disabled_scenario_config_disabled() {
        let s = FaultScenario::default();
        let config = s.to_fault_config();
        assert!(!config.enabled);
    }

    #[test]
    fn burst_failure_schedule() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 50);
        assert_eq!(sched.events.len(), 1);
        assert_eq!(sched.events[0].tick, 100);
        match &sched.events[0].action {
            ScheduledAction::KillRandomAgents(n) => assert_eq!(*n, 10), // 20% of 50
            _ => panic!("expected KillRandomAgents"),
        }
    }

    #[test]
    fn burst_failure_clamps_to_duration() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 10.0,
            burst_at_tick: 9999,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 20);
        assert_eq!(sched.events[0].tick, 500);
    }

    #[test]
    fn burst_failure_min_one_kill() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 0.1, // very low
            burst_at_tick: 100,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 10);
        match &sched.events[0].action {
            ScheduledAction::KillRandomAgents(n) => assert!(*n >= 1),
            _ => panic!("expected KillRandomAgents"),
        }
    }

    #[test]
    fn wear_based_config_uses_weibull() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::WearBased,
            wear_heat_rate: WearHeatRate::High,
            wear_threshold: 60.0,
            ..Default::default()
        };
        let config = s.to_fault_config();
        assert!(config.enabled);
        assert!(config.weibull_enabled);
        assert!(!config.intermittent_enabled);
        let (beta, eta) = WearHeatRate::High.weibull_params();
        assert_eq!(config.weibull_beta, beta);
        assert_eq!(config.weibull_eta, eta);
    }

    #[test]
    fn intermittent_fault_config_enabled() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::IntermittentFault,
            intermittent_mtbf_ticks: 80,
            intermittent_recovery_ticks: 15,
            ..Default::default()
        };
        let config = s.to_fault_config();
        assert!(config.enabled);
        assert!(!config.weibull_enabled);
        assert!(config.intermittent_enabled);
        assert_eq!(config.intermittent_mtbf_ticks, 80);
        assert_eq!(config.intermittent_recovery_ticks, 15);
    }

    #[test]
    fn intermittent_fault_no_scheduled_events() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::IntermittentFault,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 30);
        assert!(sched.events.is_empty());
    }

    #[test]
    fn wear_based_no_scheduled_events() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::WearBased,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 30);
        assert!(sched.events.is_empty());
    }

    #[test]
    fn wear_heat_rate_multipliers() {
        assert!(WearHeatRate::Low.multiplier() < WearHeatRate::Medium.multiplier());
        assert!(WearHeatRate::Medium.multiplier() < WearHeatRate::High.multiplier());
    }

    #[test]
    fn zone_outage_schedule() {
        let s = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::ZoneOutage,
            zone_at_tick: 200,
            zone_latency_duration: 50,
            ..Default::default()
        };
        let sched = s.generate_schedule(500, 30);
        assert_eq!(sched.events.len(), 1);
        assert_eq!(sched.events[0].tick, 200);
        match &sched.events[0].action {
            ScheduledAction::ZoneLatency { duration } => assert_eq!(*duration, 50),
            _ => panic!("expected ZoneLatency"),
        }
    }

    #[test]
    fn disabled_scenario_empty_schedule() {
        let s = FaultScenario::default();
        let sched = s.generate_schedule(500, 30);
        assert!(sched.events.is_empty());
    }

    #[test]
    fn scenario_type_roundtrip() {
        for id in ["burst_failure", "wear_based", "zone_outage", "intermittent_fault"] {
            let t: FaultScenarioType = id.parse().unwrap();
            assert_eq!(t.id(), id, "roundtrip failed for {id}");
        }
    }

    // --- FaultList compile tests ---

    #[test]
    fn fault_list_empty_produces_disabled() {
        let list = FaultList::default();
        let (config, sched) = list.compile(500, 20);
        assert!(!config.enabled);
        assert!(sched.events.is_empty());
    }

    #[test]
    fn fault_list_single_burst() {
        let list = FaultList {
            items: vec![FaultItem {
                fault_type: FaultScenarioType::BurstFailure,
                burst_kill_percent: 20.0,
                burst_at_tick: 100,
                ..Default::default()
            }],
        };
        let (config, sched) = list.compile(500, 50);
        assert!(config.enabled);
        assert_eq!(sched.events.len(), 1);
        assert_eq!(sched.events[0].tick, 100);
        match &sched.events[0].action {
            ScheduledAction::KillRandomAgents(n) => assert_eq!(*n, 10),
            _ => panic!("expected KillRandomAgents"),
        }
    }

    #[test]
    fn fault_list_same_tick_bursts_merge() {
        let list = FaultList {
            items: vec![
                FaultItem {
                    fault_type: FaultScenarioType::BurstFailure,
                    burst_kill_percent: 20.0,
                    burst_at_tick: 100,
                    ..Default::default()
                },
                FaultItem {
                    fault_type: FaultScenarioType::BurstFailure,
                    burst_kill_percent: 30.0,
                    burst_at_tick: 100,
                    ..Default::default()
                },
            ],
        };
        let (_, sched) = list.compile(500, 20);
        assert_eq!(sched.events.len(), 1); // merged
        match &sched.events[0].action {
            ScheduledAction::KillRandomAgents(n) => assert_eq!(*n, 10), // 50% of 20
            _ => panic!("expected KillRandomAgents"),
        }
    }

    #[test]
    fn fault_list_different_tick_bursts_separate() {
        let list = FaultList {
            items: vec![
                FaultItem {
                    fault_type: FaultScenarioType::BurstFailure,
                    burst_kill_percent: 20.0,
                    burst_at_tick: 100,
                    ..Default::default()
                },
                FaultItem {
                    fault_type: FaultScenarioType::BurstFailure,
                    burst_kill_percent: 30.0,
                    burst_at_tick: 200,
                    ..Default::default()
                },
            ],
        };
        let (_, sched) = list.compile(500, 20);
        assert_eq!(sched.events.len(), 2);
        assert_eq!(sched.events[0].tick, 100);
        assert_eq!(sched.events[1].tick, 200);
    }

    #[test]
    fn fault_list_zone_same_tick_max_duration() {
        let list = FaultList {
            items: vec![
                FaultItem {
                    fault_type: FaultScenarioType::ZoneOutage,
                    zone_at_tick: 150,
                    zone_latency_duration: 30,
                    ..Default::default()
                },
                FaultItem {
                    fault_type: FaultScenarioType::ZoneOutage,
                    zone_at_tick: 150,
                    zone_latency_duration: 80,
                    ..Default::default()
                },
            ],
        };
        let (_, sched) = list.compile(500, 20);
        assert_eq!(sched.events.len(), 1);
        match &sched.events[0].action {
            ScheduledAction::ZoneLatency { duration } => assert_eq!(*duration, 80),
            _ => panic!("expected ZoneLatency"),
        }
    }

    #[test]
    fn fault_list_combined_burst_wear_zone() {
        let list = FaultList {
            items: vec![
                FaultItem { fault_type: FaultScenarioType::BurstFailure, burst_kill_percent: 10.0, burst_at_tick: 50, ..Default::default() },
                FaultItem { fault_type: FaultScenarioType::WearBased, wear_heat_rate: WearHeatRate::High, ..Default::default() },
                FaultItem { fault_type: FaultScenarioType::ZoneOutage, zone_at_tick: 200, zone_latency_duration: 40, ..Default::default() },
            ],
        };
        let (config, sched) = list.compile(500, 30);
        assert!(config.enabled);
        assert!(config.weibull_enabled);
        assert_eq!(sched.events.len(), 2); // burst + zone
        assert_eq!(sched.events[0].tick, 50);
        assert_eq!(sched.events[1].tick, 200);
    }

    #[test]
    fn fault_list_wear_only_first_used() {
        let list = FaultList {
            items: vec![
                FaultItem { fault_type: FaultScenarioType::WearBased, wear_heat_rate: WearHeatRate::Low, ..Default::default() },
                FaultItem { fault_type: FaultScenarioType::WearBased, wear_heat_rate: WearHeatRate::High, ..Default::default() },
            ],
        };
        let (config, _) = list.compile(500, 20);
        // First wear item wins
        let (beta, eta) = WearHeatRate::Low.weibull_params();
        assert_eq!(config.weibull_beta, beta);
        assert_eq!(config.weibull_eta, eta);
    }

    #[test]
    fn schedule_clear() {
        let mut sched = FaultSchedule {
            events: vec![ScheduledEvent {
                tick: 100,
                action: ScheduledAction::KillRandomAgents(1),
                fired: false,
            }],
            initialized: true,
        };
        sched.clear();
        assert!(sched.events.is_empty());
        assert!(!sched.initialized);
    }
}
