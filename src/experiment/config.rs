//! Experiment configuration: single-run identity and batch matrix.

use crate::core::grid::GridMap;
use crate::core::topology::ZoneMap;
use crate::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};

/// Identity of a single experiment run.
#[derive(Debug, Clone)]
pub struct ExperimentConfig {
    pub solver_name: String,
    pub topology_name: String,
    pub scenario: Option<FaultScenario>,
    pub scheduler_name: String,
    pub num_agents: usize,
    pub seed: u64,
    pub tick_count: u64,
    /// Inline map data for custom/imported maps. When present, used instead of
    /// `topology_name` lookup via `ActiveTopology::from_name()`.
    pub custom_map: Option<(GridMap, ZoneMap)>,
}

impl ExperimentConfig {
    /// Human-readable label for this config (used in CSV output and grouping).
    /// Includes intensity parameters so burst_20 ≠ burst_50, wear_medium ≠ wear_high.
    pub fn scenario_label(&self) -> String {
        match &self.scenario {
            None => "none".to_string(),
            Some(s) => match s.scenario_type {
                FaultScenarioType::BurstFailure => {
                    format!("burst_{}pct", s.burst_kill_percent as u32)
                }
                FaultScenarioType::WearBased => {
                    format!("wear_{}", s.wear_heat_rate.id())
                }
                FaultScenarioType::ZoneOutage => {
                    format!("zone_{}t", s.zone_latency_duration)
                }
                FaultScenarioType::IntermittentFault => {
                    format!("intermittent_{}m{}r", s.intermittent_mtbf_ticks, s.intermittent_recovery_ticks)
                }
                FaultScenarioType::PermanentZoneOutage => {
                    format!("perm_zone_{}pct", s.perm_zone_block_percent as u32)
                }
            },
        }
    }
}

/// Batch experiment definition — expands to Cartesian product of configs.
#[derive(Debug, Clone)]
pub struct ExperimentMatrix {
    pub solvers: Vec<String>,
    pub topologies: Vec<String>,
    pub scenarios: Vec<Option<FaultScenario>>,
    pub schedulers: Vec<String>,
    pub agent_counts: Vec<usize>,
    pub seeds: Vec<u64>,
    pub tick_count: u64,
}

impl ExperimentMatrix {
    /// Expand into the full Cartesian product of experiment configs.
    pub fn expand(&self) -> Vec<ExperimentConfig> {
        let mut configs = Vec::new();
        for solver in &self.solvers {
            for topology in &self.topologies {
                for scenario in &self.scenarios {
                    for scheduler in &self.schedulers {
                        for &num_agents in &self.agent_counts {
                            for &seed in &self.seeds {
                                configs.push(ExperimentConfig {
                                    solver_name: solver.clone(),
                                    topology_name: topology.clone(),
                                    scenario: scenario.clone(),
                                    scheduler_name: scheduler.clone(),
                                    num_agents,
                                    seed,
                                    tick_count: self.tick_count,
                                    custom_map: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        configs
    }

    /// Total number of runs this matrix will produce.
    pub fn total_runs(&self) -> usize {
        self.solvers.len()
            * self.topologies.len()
            * self.scenarios.len()
            * self.schedulers.len()
            * self.agent_counts.len()
            * self.seeds.len()
    }
}

/// Helper to build common fault scenarios for experiment matrices.
pub fn standard_scenarios() -> Vec<Option<FaultScenario>> {
    vec![
        // No faults (baseline-only run for comparison)
        None,
        // Burst: kill 20% at tick 100
        Some(FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            ..Default::default()
        }),
        // Wear-based: medium heat rate
        Some(FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::WearBased,
            wear_heat_rate: WearHeatRate::Medium,
            wear_threshold: 80.0,
            ..Default::default()
        }),
        // Zone outage at tick 100, 50 ticks duration
        Some(FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::ZoneOutage,
            zone_at_tick: 100,
            zone_latency_duration: 50,
            ..Default::default()
        }),
        // Intermittent fault: 80-tick MTBF, 15-tick recovery
        Some(FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::IntermittentFault,
            intermittent_mtbf_ticks: 80,
            intermittent_recovery_ticks: 15,
            ..Default::default()
        }),
        // Permanent zone outage: 100% block at tick 100
        Some(FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::PermanentZoneOutage,
            perm_zone_at_tick: 100,
            perm_zone_block_percent: 100.0,
            ..Default::default()
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_cartesian_product() {
        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_medium".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![10, 20],
            seeds: vec![1, 2, 3],
            tick_count: 100,
        };
        let configs = matrix.expand();
        assert_eq!(configs.len(), 6); // 1×1×1×1×2×3
        assert_eq!(matrix.total_runs(), 6);
    }

    #[test]
    fn expand_full_product() {
        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into(), "rhcr_pibt".into()],
            topologies: vec!["compact_grid".into(), "warehouse_medium".into()],
            scenarios: standard_scenarios(),
            schedulers: vec!["random".into()],
            agent_counts: vec![20],
            seeds: vec![42],
            tick_count: 500,
        };
        // 2 x 2 x 6 x 1 x 1 x 1 = 24
        assert_eq!(matrix.total_runs(), 24);
        let configs = matrix.expand();
        assert_eq!(configs.len(), 24);
    }

    #[test]
    fn scenario_label() {
        let cfg = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_medium".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };
        assert_eq!(cfg.scenario_label(), "none");

        let cfg2 = ExperimentConfig {
            scenario: Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::BurstFailure,
                burst_kill_percent: 20.0,
                ..Default::default()
            }),
            ..cfg
        };
        assert_eq!(cfg2.scenario_label(), "burst_20pct");
    }

    #[test]
    fn standard_scenarios_count() {
        let scenarios = standard_scenarios();
        assert_eq!(scenarios.len(), 6); // none + 5 fault types
        assert!(scenarios[0].is_none());
        assert!(scenarios[1].is_some());
    }

    #[test]
    fn empty_matrix_produces_empty() {
        let matrix = ExperimentMatrix {
            solvers: vec![],
            topologies: vec!["warehouse_medium".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![10],
            seeds: vec![42],
            tick_count: 100,
        };
        assert_eq!(matrix.total_runs(), 0);
        assert!(matrix.expand().is_empty());
    }
}
