//! Reliability regression tests for the paired-baseline protocol:
//! RNG stream isolation, wear clock, queue goal validity, collision accounting.

use bevy::math::IVec2;
use rand::Rng;

use mafis::analysis::baseline::place_agents;
use mafis::core::queue::ActiveQueuePolicy;
use mafis::core::runner::{SimCommand, SimulationRunner};
use mafis::core::seed::SeededRng;
use mafis::core::task::ActiveScheduler;
use mafis::core::topology::ActiveTopology;
use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::config::{FaultConfig, FaultSource};
use mafis::fault::scenario::{
    FaultScenario, FaultScenarioType, FaultSchedule, ScheduledAction, ScheduledEvent,
};
use mafis::solver::heuristics::DistanceMapCache;
use mafis::solver::lifelong::{AgentState, LifelongSolver, SolverContext, StepResult};
use mafis::solver::traits::SolverInfo;

const SOLVERS: [&str; 3] = ["pibt", "rhcr_pbs", "token_passing"];
const TOPOLOGY: &str = "warehouse_single_dock";

fn runner_with(
    solver: Box<dyn LifelongSolver>,
    n: usize,
    seed: u64,
    fault_config: FaultConfig,
    schedule: FaultSchedule,
) -> SimulationRunner {
    let output = ActiveTopology::from_name(TOPOLOGY).topology().generate(seed);
    let mut rng = SeededRng::new(seed);
    let agents = place_agents(n, &output.grid, &output.zones, &mut rng);
    SimulationRunner::new(output.grid, output.zones, agents, solver, rng, fault_config, schedule)
}

fn solver(name: &str, n: usize) -> Box<dyn LifelongSolver> {
    let output = ActiveTopology::from_name(TOPOLOGY).topology().generate(0);
    let area = (output.grid.width * output.grid.height) as usize;
    mafis::solver::lifelong_solver_from_name(name, area, n).expect("known solver")
}

fn no_faults() -> FaultConfig {
    FaultConfig { enabled: false, ..Default::default() }
}

fn tick(runner: &mut SimulationRunner) {
    let scheduler = ActiveScheduler::from_name("closest");
    let policy = ActiveQueuePolicy::from_name("closest");
    runner.tick(scheduler.scheduler(), policy.policy());
}

fn positions(runner: &SimulationRunner) -> Vec<(IVec2, bool)> {
    runner.agents.iter().map(|a| (a.pos, a.alive)).collect()
}

// ── 1. RNG stream isolation ─────────────────────────────────────────

/// PIBT wrapper that burns a tick-dependent number of draws from the solver
/// RNG after planning. If the solver stream is shared with task demand or
/// carried across ticks, the trajectory diverges from plain PIBT.
struct NoisyPibt {
    inner: Box<dyn LifelongSolver>,
}

impl LifelongSolver for NoisyPibt {
    fn name(&self) -> &'static str {
        "noisy_pibt"
    }
    fn info(&self) -> SolverInfo {
        self.inner.info()
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn step<'a>(
        &'a mut self,
        ctx: &SolverContext,
        agents: &[AgentState],
        distance_cache: &mut DistanceMapCache,
        rng: &mut SeededRng,
    ) -> StepResult<'a> {
        let result = self.inner.step(ctx, agents, distance_cache, rng);
        for _ in 0..(ctx.tick % 5) {
            let _: u64 = rng.rng.random();
        }
        result
    }
}

#[test]
fn solver_rng_consumption_does_not_leak_into_other_streams() {
    let n = 40;
    let mut plain = runner_with(solver("pibt", n), n, 42, no_faults(), FaultSchedule::default());
    let mut noisy = runner_with(
        Box::new(NoisyPibt { inner: solver("pibt", n) }),
        n,
        42,
        no_faults(),
        FaultSchedule::default(),
    );
    for t in 1..=300 {
        tick(&mut plain);
        tick(&mut noisy);
        assert_eq!(positions(&plain), positions(&noisy), "diverged at tick {t}");
    }
}

#[test]
fn burst_equals_direct_kill_of_the_same_agents() {
    // A burst must perturb only the agents it kills: victim selection may not
    // consume the task-demand stream.
    let n = 40;
    let burst_tick = 100;
    for name in SOLVERS {
        let schedule = FaultSchedule {
            events: vec![ScheduledEvent {
                tick: burst_tick,
                action: ScheduledAction::KillRandomAgents(8),
                fired: false,
            }],
            initialized: true,
        };
        let mut burst = runner_with(solver(name, n), n, 42, no_faults(), schedule);
        let mut direct =
            runner_with(solver(name, n), n, 42, no_faults(), FaultSchedule::default());

        for _ in 1..burst_tick {
            tick(&mut burst);
            tick(&mut direct);
        }
        tick(&mut burst);
        let victims: Vec<usize> = (0..n).filter(|&i| !burst.agents[i].alive).collect();
        assert_eq!(victims.len(), 8);
        for &index in &victims {
            direct.enqueue_command(SimCommand::KillAgent { index, source: FaultSource::Manual });
        }
        tick(&mut direct);
        assert_eq!(positions(&burst), positions(&direct), "{name}: diverged at burst tick");

        for t in burst_tick + 1..=burst_tick + 200 {
            tick(&mut burst);
            tick(&mut direct);
            assert_eq!(positions(&burst), positions(&direct), "{name}: diverged at tick {t}");
        }
    }
}

fn zero_magnitude_config(solver_name: &str, scenario: FaultScenario) -> ExperimentConfig {
    ExperimentConfig {
        solver_name: solver_name.into(),
        topology_name: TOPOLOGY.into(),
        scenario: Some(scenario),
        scheduler_name: "closest".into(),
        num_agents: 40,
        seed: 42,
        tick_count: 300,
        custom_map: None,
        rhcr_override: None,
    }
}

#[test]
fn zero_magnitude_faults_leave_the_run_identical() {
    let burst_zero = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 0.0,
        ..Default::default()
    };
    let zone_zero = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::ZoneOutage,
        zone_latency_duration: 0,
        ..Default::default()
    };
    for name in SOLVERS {
        for scenario in [burst_zero.clone(), zone_zero.clone()] {
            let label = format!("{name} {:?}", scenario.scenario_type);
            let r = run_single_experiment(&zero_magnitude_config(name, scenario));
            let (b, f) = (&r.baseline_metrics, &r.faulted_metrics);
            assert_eq!(f.total_tasks, b.total_tasks, "{label}: task count changed");
            assert_eq!(f.survival_rate, 1.0, "{label}: an agent died");
            if f.fault_tolerance.is_finite() {
                assert_eq!(f.fault_tolerance, 1.0, "{label}: FT");
            }
            if f.itae.is_finite() {
                assert_eq!(f.itae, 0.0, "{label}: TWTE");
            }
        }
    }
}

// ── 3. Wear clock ───────────────────────────────────────────────────

#[test]
fn wear_deaths_happen_at_the_same_tick_under_every_solver() {
    let n = 40;
    let wear = FaultConfig {
        enabled: true,
        weibull_enabled: true,
        weibull_beta: 2.5,
        weibull_eta: 500.0,
        intermittent_enabled: false,
        ..Default::default()
    };
    let deaths_under = |name: &str| -> Vec<(usize, u64)> {
        let mut runner = runner_with(solver(name, n), n, 42, wear.clone(), FaultSchedule::default());
        let mut deaths = Vec::new();
        for _ in 0..300 {
            tick(&mut runner);
            for (i, a) in runner.agents.iter().enumerate() {
                if !a.alive && !deaths.iter().any(|&(j, _)| j == i) {
                    deaths.push((i, runner.tick));
                }
            }
        }
        deaths
    };
    let reference = deaths_under("pibt");
    assert!(!reference.is_empty(), "Wear-Medium should kill some agents within 300 ticks");
    for name in ["rhcr_pbs", "token_passing"] {
        assert_eq!(deaths_under(name), reference, "{name}: death schedule differs from PIBT");
    }
}

// ── 4. Queue goals ──────────────────────────────────────────────────

#[test]
fn no_alive_agent_targets_a_blocked_cell_after_a_burst() {
    let n = 40;
    for name in SOLVERS {
        let schedule = FaultSchedule {
            events: vec![ScheduledEvent {
                tick: 100,
                action: ScheduledAction::KillRandomAgents(20),
                fired: false,
            }],
            initialized: true,
        };
        let mut runner = runner_with(solver(name, n), n, 42, no_faults(), schedule);
        for _ in 0..400 {
            tick(&mut runner);
            for (i, a) in runner.agents.iter().enumerate() {
                assert!(
                    !a.alive || runner.grid.is_walkable(a.goal),
                    "{name} tick {}: agent {i} ({:?}) targets blocked cell {:?}",
                    runner.tick,
                    a.task_leg,
                    a.goal
                );
            }
        }
    }
}

#[test]
fn fault_free_runs_never_eject_queued_agents() {
    let n = 40;
    for name in SOLVERS {
        let mut runner = runner_with(solver(name, n), n, 42, no_faults(), FaultSchedule::default());
        for _ in 0..300 {
            tick(&mut runner);
        }
        assert_eq!(runner.queue_reroutes(), 0, "{name}: queued agents ejected without faults");
    }
}

// ── 5. Collision accounting ─────────────────────────────────────────

#[test]
fn runs_export_zero_violations_and_pibt_never_needs_the_resolver() {
    for name in SOLVERS {
        let r = run_single_experiment(&ExperimentConfig {
            scenario: None,
            ..zero_magnitude_config(name, FaultScenario::default())
        });
        let m = &r.baseline_metrics;
        assert_eq!(m.collision_violations, 0, "{name}: post-hoc collision");
        eprintln!("{name}: forced_waits = {}", m.forced_waits);
        if name == "pibt" {
            assert_eq!(m.forced_waits, 0, "pibt: resolver overrode a PIBT move");
        }
    }
}
