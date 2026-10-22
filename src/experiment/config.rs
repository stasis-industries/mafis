//! Experiment configuration: single-run identity and batch matrix.

use crate::core::grid::GridMap;
use crate::core::topology::ZoneMap;
use crate::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};
use crate::planning::params::ParamValues;
use crate::planning::registry;

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
    /// Parameters for the planner; a key left out takes its declared default.
    pub planner_params: ParamValues,
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
                FaultScenarioType::NullPerturb => "null_perturb".to_string(),
                FaultScenarioType::IntermittentFault => {
                    // Format: intermittent_{start}s{mtbf}m{recovery}r
                    // `start` = warm-up floor tick (0 = no warm-up).
                    format!(
                        "intermittent_{}s{}m{}r",
                        s.intermittent_start_tick,
                        s.intermittent_mtbf_ticks,
                        s.intermittent_recovery_ticks
                    )
                }
            },
        }
    }

    /// The planner's parameters as canonical text (every declared key, with
    /// defaults filled in), as recorded in results and in the twin key.
    pub fn planner_params_text(&self) -> String {
        registry::resolve(&self.solver_name, &self.planner_params)
            .map(|(_, params)| params.canonical())
            .unwrap_or_else(|e| panic!("{e}"))
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
    /// Planner parameter sets, each crossed with every planner. The default is
    /// one empty set (every planner's defaults); add sets to sweep parameters.
    pub planner_params: Vec<ParamValues>,
}

impl Default for ExperimentMatrix {
    fn default() -> Self {
        Self {
            solvers: Vec::new(),
            topologies: Vec::new(),
            scenarios: Vec::new(),
            schedulers: Vec::new(),
            agent_counts: Vec::new(),
            seeds: Vec::new(),
            tick_count: 0,
            planner_params: vec![Default::default()],
        }
    }
}

impl ExperimentMatrix {
    /// Expand into the Cartesian product of experiment configs, skipping
    /// cells whose fleet exceeds the planner's fleet limit on that topology
    /// (with a warning) so no matrix runs a planner outside its supported range.
    /// Panics on an unknown planner or a parameter set it does not accept.
    pub fn expand(&self) -> Vec<ExperimentConfig> {
        let mut configs = Vec::new();
        let default_params = [ParamValues::default()];
        let param_sets: &[ParamValues] =
            if self.planner_params.is_empty() { &default_params } else { &self.planner_params };
        for solver in &self.solvers {
            for params in param_sets {
                if let Err(e) = registry::resolve(solver, params) {
                    panic!("{e}");
                }
            }
            for topology in &self.topologies {
                let cap = solver_agent_cap(solver, topology);
                #[cfg(not(target_arch = "wasm32"))]
                for &n in self.agent_counts.iter().filter(|&&n| cap.is_some_and(|c| n > c)) {
                    eprintln!(
                        "WARNING: skipping {solver} at {n} agents on {topology} (max {})",
                        cap.unwrap_or_default()
                    );
                }
                for scenario in &self.scenarios {
                    for scheduler in &self.schedulers {
                        for &num_agents in &self.agent_counts {
                            if cap.is_some_and(|c| num_agents > c) {
                                continue;
                            }
                            for &seed in &self.seeds {
                                for params in param_sets {
                                    configs.push(ExperimentConfig {
                                        solver_name: solver.clone(),
                                        topology_name: topology.clone(),
                                        scenario: scenario.clone(),
                                        scheduler_name: scheduler.clone(),
                                        num_agents,
                                        seed,
                                        tick_count: self.tick_count,
                                        custom_map: None,
                                        planner_params: params.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        configs
    }

    /// Total number of runs this matrix will produce (same skips as `expand`).
    pub fn total_runs(&self) -> usize {
        let param_sets = self.planner_params.len().max(1);
        let mut cells = 0;
        for solver in &self.solvers {
            for topology in &self.topologies {
                let cap = solver_agent_cap(solver, topology);
                cells += self.agent_counts.iter().filter(|&&n| cap.is_none_or(|c| n <= c)).count();
            }
        }
        cells * self.scenarios.len() * self.schedulers.len() * self.seeds.len() * param_sets
    }
}

/// Largest fleet `solver` supports on `topology`, from the planner's registered
/// `fleet_limit` (`None` = uncapped, unknown planner or unknown topology; the
/// run itself then fails loudly). Memoized: the desktop UI recounts runs
/// every frame and topologies do not change at runtime.
fn solver_agent_cap(solver: &str, topology: &str) -> Option<usize> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CAPS: OnceLock<Mutex<HashMap<(String, String), Option<usize>>>> = OnceLock::new();
    let key = (solver.to_string(), topology.to_string());
    let caps = CAPS.get_or_init(Default::default);
    if let Some(cap) = caps.lock().ok().and_then(|m| m.get(&key).copied()) {
        return cap;
    }
    let cap =
        registry::find(solver).and_then(|entry| (entry.fleet_limit)(&topology_zones(topology)?));
    if let Ok(mut m) = caps.lock() {
        m.insert(key, cap);
    }
    cap
}

#[cfg(not(target_arch = "wasm32"))]
fn topology_zones(topology: &str) -> Option<ZoneMap> {
    use crate::core::topology::TopologyRegistry;
    let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
    TopologyRegistry::parse_entry(registry.find(topology)?).map(|(_, zones)| zones)
}

/// shortcut: WASM has no topology registry on disk here, so only solver caps
/// that ignore the map apply; upgrade if matrices ever expand inside WASM.
#[cfg(target_arch = "wasm32")]
fn topology_zones(_topology: &str) -> Option<ZoneMap> {
    Some(ZoneMap::default())
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::params::ParamValue;

    #[test]
    #[should_panic(expected = "unknown parameter")]
    fn matrix_with_undeclared_param_panics() {
        let matrix = ExperimentMatrix {
            solvers: vec![crate::planners::DEFAULT.into()],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![10],
            seeds: vec![1],
            tick_count: 10,
            planner_params: vec![ParamValues::default().with("undeclared_key", ParamValue::Int(1))],
        };
        matrix.expand();
    }

    #[test]
    fn expand_cartesian_product() {
        let matrix = ExperimentMatrix {
            solvers: vec![crate::planners::DEFAULT.into()],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![10, 20],
            seeds: vec![1, 2, 3],
            tick_count: 100,
            planner_params: vec![Default::default()],
        };
        let configs = matrix.expand();
        assert_eq!(configs.len(), 6); // 1×1×1×1×2×3×1
        assert_eq!(matrix.total_runs(), 6);
    }

    #[test]
    fn expand_skips_cells_above_a_fleet_limit() {
        let zones = topology_zones("warehouse_dual_dock").expect("the map exists");
        let (entry, limit) = registry::entries()
            .iter()
            .find_map(|entry| (entry.fleet_limit)(&zones).map(|limit| (entry, limit)))
            .expect("a registered planner has a fleet limit on this map");
        let matrix = ExperimentMatrix {
            solvers: vec![entry.info.id.into()],
            topologies: vec!["warehouse_dual_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["closest".into()],
            agent_counts: vec![limit, limit + 1],
            seeds: vec![1],
            tick_count: 100,
            planner_params: vec![Default::default()],
        };
        let fleets: Vec<usize> = matrix.expand().iter().map(|c| c.num_agents).collect();
        assert_eq!(fleets, vec![limit]);
        assert_eq!(matrix.total_runs(), 1);
    }

    #[test]
    fn expand_full_product() {
        let matrix = ExperimentMatrix {
            solvers: vec![crate::planners::DEFAULT.into()],
            topologies: vec!["compact_grid".into(), "warehouse_single_dock".into()],
            scenarios: standard_scenarios(),
            schedulers: vec!["random".into()],
            agent_counts: vec![20],
            seeds: vec![42],
            tick_count: 500,
            planner_params: vec![Default::default()],
        };
        // 1 x 2 x 5 x 1 x 1 x 1 x 1 = 10
        assert_eq!(matrix.total_runs(), 10);
        let configs = matrix.expand();
        assert_eq!(configs.len(), 10);
    }

    #[test]
    fn expand_with_planner_params_multiplies_axes() {
        use crate::planning::params::ParamKind;
        let entry = registry::entries()
            .iter()
            .find(|entry| !entry.info.params.is_empty())
            .expect("a registered planner declares a parameter");
        let spec = entry.info.params[0];
        let ParamKind::Int { min, max, default } = spec.kind;
        let matrix = ExperimentMatrix {
            solvers: vec![entry.info.id.into()],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["closest".into()],
            agent_counts: vec![20],
            seeds: vec![1, 2],
            tick_count: 100,
            planner_params: [min, default, max]
                .map(|v| ParamValues::default().with(spec.key, ParamValue::Int(v)))
                .to_vec(),
        };
        assert_eq!(matrix.total_runs(), 6);
        let configs = matrix.expand();
        assert_eq!(configs.len(), 6);
        let texts: std::collections::BTreeSet<String> =
            configs.iter().map(|c| c.planner_params_text()).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
    }

    #[test]
    fn scenario_label() {
        let cfg = ExperimentConfig {
            solver_name: crate::planners::DEFAULT.into(),
            topology_name: "warehouse_single_dock".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
            planner_params: Default::default(),
        };
        assert_eq!(cfg.scenario_label(), "none");
        assert_eq!(cfg.planner_params_text(), "");

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
        assert_eq!(scenarios.len(), 5); // none + 4 fault types
        assert!(scenarios[0].is_none());
        assert!(scenarios[1].is_some());
    }

    #[test]
    fn empty_matrix_produces_empty() {
        let matrix = ExperimentMatrix {
            solvers: vec![],
            topologies: vec!["warehouse_single_dock".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![10],
            seeds: vec![42],
            tick_count: 100,
            planner_params: vec![Default::default()],
        };
        assert_eq!(matrix.total_runs(), 0);
        assert!(matrix.expand().is_empty());
    }
}
