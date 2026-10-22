//! Golden runs: every built-in planner on every map, hashed tick by tick.
//!
//! A change that must not alter behaviour keeps every hash in
//! `tests/golden/runs.txt`. Re-bless only when a change is meant to alter runs.
//! Faults are limited to burst and zone outage: wear and intermittent sample
//! through `ln`/`powf`, which can differ between macOS and the Linux CI.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mafis::analysis::baseline::place_agents;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::SimulationRunner;
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::{ActiveTopology, TopologyRegistry};
use mafis::fault::config::FaultConfig;
use mafis::fault::scenario::{FaultScenario, FaultScenarioType, FaultSchedule};

const PLANNERS: [&str; 3] = ["pibt", "rhcr_pbs", "token_passing"];
const MAPS: [&str; 8] = [
    "warehouse_medium",
    "warehouse_single_dock",
    "warehouse_single_dock_w2",
    "warehouse_single_dock_w3",
    "compact_grid",
    "warehouse_dual_dock",
    "sorting_center",
    "fulfilment_center",
];
const SEED: u64 = 42;
const TICKS: u64 = 100;
const SMALL_FLEET: usize = 8;
const LARGE_FLEET_CAP: usize = 20;
const FAULT_TICK: u64 = 40;
const BURST_PERCENT: f32 = 20.0;
const ZONE_TICKS: u32 = 30;
const BLESS_VAR: &str = "MAFIS_BLESS_GOLDEN";
const BLESS_CMD: &str = "MAFIS_BLESS_GOLDEN=1 cargo test --test golden_runs";

#[derive(Clone, Copy)]
enum Variant {
    Twin,
    Burst,
    Zone,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::Twin, Variant::Burst, Variant::Zone];

    fn name(self) -> &'static str {
        match self {
            Variant::Twin => "twin",
            Variant::Burst => "burst",
            Variant::Zone => "zone",
        }
    }

    fn faults(self, n: usize) -> (FaultConfig, FaultSchedule) {
        let scenario = match self {
            Variant::Twin => {
                return (
                    FaultConfig { enabled: false, ..Default::default() },
                    FaultSchedule::default(),
                );
            }
            Variant::Burst => FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::BurstFailure,
                burst_kill_percent: BURST_PERCENT,
                burst_at_tick: FAULT_TICK,
                ..Default::default()
            },
            Variant::Zone => FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::ZoneOutage,
                zone_at_tick: FAULT_TICK,
                zone_latency_duration: ZONE_TICKS,
                ..Default::default()
            },
        };
        (scenario.to_fault_config(), scenario.generate_schedule(TICKS, n))
    }
}

/// FNV-1a 64: stable across platforms and Rust versions, unlike `DefaultHasher`.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The golden line for one cell, or `None` when the planner's fleet limit excludes it.
/// Built like the faulted run in `run_single_experiment`.
fn golden_line(planner: &str, map: &str, n: usize, variant: Variant) -> Option<String> {
    let output = ActiveTopology::from_name(map).topology().generate(SEED);
    let area = (output.grid.width * output.grid.height) as usize;
    let solver = mafis::solver::lifelong_solver_from_name(planner, area, n)
        .unwrap_or_else(|| panic!("unknown planner '{planner}'"));
    if solver.max_agents(&output.zones).is_some_and(|cap| cap < n) {
        return None;
    }

    let mut rng = SeededRng::new(SEED);
    let agents = place_agents(n, &output.grid, &output.zones, &mut rng);
    let (fault_config, schedule) = variant.faults(n);
    let mut runner = SimulationRunner::new(
        output.grid,
        output.zones,
        agents,
        solver,
        rng,
        fault_config,
        schedule,
    );
    let scheduler = ActiveScheduler::from_name("closest");
    let policy = ActiveQueuePolicy::from_name("closest");

    let mut hash = Fnv::new();
    for tick in 1..=TICKS {
        runner.tick(scheduler.scheduler(), policy.policy());
        hash.write(&tick.to_le_bytes());
        for agent in &runner.agents {
            hash.write(&agent.pos.x.to_le_bytes());
            hash.write(&agent.pos.y.to_le_bytes());
            hash.write(&[u8::from(agent.alive), agent.last_action.to_u8()]);
        }
    }
    hash.write(&runner.tasks_completed.to_le_bytes());

    Some(format!(
        "{planner} {map} {n} {} {:016x} {}",
        variant.name(),
        hash.0,
        runner.tasks_completed
    ))
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/runs.txt")
}

/// One message per expected line absent from `actual`, one per line `actual` adds.
fn compare(expected: &BTreeSet<String>, actual: &BTreeSet<String>) -> Vec<String> {
    let missing = expected.difference(actual).map(|line| format!("missing: {line}"));
    let extra = actual.difference(expected).map(|line| format!("extra: {line}"));
    missing.chain(extra).collect()
}

#[test]
fn compare_reports_missing_and_extra() {
    let e: BTreeSet<String> = ["a 1".to_string(), "b 2".to_string()].into();
    let a: BTreeSet<String> = ["b 2".to_string(), "c 3".to_string()].into();
    let report = compare(&e, &a);
    assert_eq!(report.len(), 2);
    assert!(report.iter().any(|m| m.contains("missing") && m.contains("a 1")));
    assert!(report.iter().any(|m| m.contains("extra") && m.contains("c 3")));
}

#[test]
fn golden_runs_match() {
    // The map's own fleet size lives on its registry entry; `TopologyOutput`
    // reports 0 for JSON maps.
    let registry = TopologyRegistry::load_from_dir(Path::new("topologies"));
    let mut actual = BTreeSet::new();
    for map in MAPS {
        let map_fleet =
            registry.find(map).unwrap_or_else(|| panic!("unknown map '{map}'")).number_agents;
        for n in [SMALL_FLEET, LARGE_FLEET_CAP.min(map_fleet)] {
            for planner in PLANNERS {
                for variant in Variant::ALL {
                    actual.extend(golden_line(planner, map, n, variant));
                }
            }
        }
    }

    let path = golden_path();
    if std::env::var(BLESS_VAR).as_deref() == Ok("1") {
        let dir = path.parent().expect("golden file has a parent directory");
        std::fs::create_dir_all(dir).expect("create tests/golden");
        let body: String = actual.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(&path, body).expect("write the golden file");
        return;
    }

    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; record it with: {BLESS_CMD}", path.display()));
    let expected: BTreeSet<String> = text.lines().map(str::to_owned).collect();
    let report = compare(&expected, &actual);
    assert!(
        report.is_empty(),
        "golden runs changed:\n{}\nre-bless only if this change is meant to alter runs: {BLESS_CMD}",
        report.join("\n")
    );
}
