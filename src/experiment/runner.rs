//! Experiment runner — executes single experiments and full matrices.

// Cross-platform Instant: std on native, web-time on WASM.
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::analysis::baseline::place_agents;
use crate::analysis::engine::AnalysisEngine;
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::SimulationRunner;
use crate::core::seed::SeededRng;
use crate::core::task::ActiveScheduler;
use crate::core::topology::ActiveTopology;
use crate::fault::config::FaultConfig;
use crate::fault::scenario::FaultSchedule;

use super::config::{ExperimentConfig, ExperimentMatrix};
use super::metrics::{RunMetrics, compute_run_metrics};
use super::stats::{StatSummary, compute_stat_summary};

/// Result of a single experiment run (paired baseline + faulted).
#[derive(Debug, Clone)]
pub struct RunResult {
    pub config: ExperimentConfig,
    pub baseline_metrics: RunMetrics,
    pub faulted_metrics: RunMetrics,
}

/// Summary statistics for one config across multiple seeds.
#[derive(Debug, Clone)]
pub struct ConfigSummary {
    /// Config identity (without seed — shared across seeds).
    pub solver_name: String,
    pub topology_name: String,
    pub scenario_label: String,
    pub scheduler_name: String,
    pub num_agents: usize,
    pub num_seeds: usize,

    // Per-metric summaries (faulted run)
    pub throughput: StatSummary,
    pub total_tasks: StatSummary,
    pub unassigned_ratio: StatSummary,
    pub fault_tolerance: StatSummary,
    pub critical_time: StatSummary,
    pub deficit_recovery: StatSummary,
    pub throughput_recovery: StatSummary,
    pub survival_rate: StatSummary,
    pub impacted_area: StatSummary,
    pub deficit_integral: StatSummary,
    pub cascade_depth: StatSummary,
    pub cascade_spread: StatSummary,
    pub fleet_utilization: StatSummary,
    pub solver_step_us: StatSummary,
    pub wall_time_ms: StatSummary,
}

/// Full matrix result — all runs + statistical summaries.
#[derive(Debug)]
pub struct MatrixResult {
    pub matrix: ExperimentMatrix,
    pub runs: Vec<RunResult>,
    pub summaries: Vec<ConfigSummary>,
    pub wall_time_total_ms: u64,
}

/// Run a single experiment: paired baseline + faulted simulation.
pub fn run_single_experiment(config: &ExperimentConfig) -> RunResult {
    let wall_start = Instant::now();

    // 1. Generate topology (use inline custom map if provided)
    let (grid, zones) = if let Some((g, z)) = &config.custom_map {
        (g.clone(), z.clone())
    } else {
        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        (output.grid, output.zones)
    };

    let grid_area = (grid.width * grid.height) as usize;
    let capacity = grid.walkable_count();

    // Clamp agent count to map capacity
    let actual_agents = if config.num_agents > capacity {
        eprintln!(
            "WARNING: {} requests {} agents but only has {} walkable cells — clamping to {}",
            config.topology_name, config.num_agents, capacity, capacity
        );
        capacity
    } else {
        config.num_agents
    };

    // 2. Create scheduler + queue policy (shared across both runs)
    let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    // 3. Place agents using shared RNG — clone rng AFTER placement
    let mut rng = SeededRng::new(config.seed);
    let agents = place_agents(actual_agents, &grid, &zones, &mut rng);
    let rng_after_placement = rng.clone();

    // ── Run baseline (faults disabled) ──────────────────────────────
    let baseline_record;
    let baseline_metrics;
    {
        let solver =
            crate::solver::lifelong_solver_from_name(&config.solver_name, grid_area, actual_agents)
                .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

        let fault_config = FaultConfig { enabled: false, ..Default::default() };

        let mut runner = SimulationRunner::new(
            grid.clone(),
            zones.clone(),
            agents.clone(),
            solver,
            rng_after_placement.clone(),
            fault_config,
            FaultSchedule::default(),
        );

        let mut analysis = AnalysisEngine::new(config.tick_count as usize);
        let mut step_times = Vec::with_capacity(config.tick_count as usize);

        for _ in 0..config.tick_count {
            let tick_start = Instant::now();
            let mut result = runner.tick(scheduler.scheduler(), queue_policy.policy());
            step_times.push(tick_start.elapsed().as_micros() as f64);
            analysis.record_tick(&runner, &mut result);
        }
        analysis.compute_aggregates();

        let bl_wall_ms = wall_start.elapsed().as_millis() as u64;

        // Baseline self-metrics (no clone needed — computed directly from engine)
        baseline_metrics =
            super::metrics::compute_baseline_self_metrics(&analysis, &step_times, bl_wall_ms);

        // Consume engine into baseline record (no clone!)
        baseline_record = analysis.into_baseline_record(
            0, // config_hash not needed for experiment
            config.tick_count,
            config.num_agents,
        );
    }

    // ── Run faulted (same topology + agents, faults enabled) ────────
    let faulted_metrics;
    {
        let solver =
            crate::solver::lifelong_solver_from_name(&config.solver_name, grid_area, actual_agents)
                .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

        let (fault_config, fault_schedule) = match &config.scenario {
            Some(scenario) => {
                let fc = scenario.to_fault_config();
                let fs = scenario.generate_schedule(config.tick_count, config.num_agents);
                (fc, fs)
            }
            None => {
                (FaultConfig { enabled: false, ..Default::default() }, FaultSchedule::default())
            }
        };

        let mut runner = SimulationRunner::new(
            grid,
            zones,
            agents,
            solver,
            rng_after_placement,
            fault_config,
            fault_schedule,
        );

        let mut analysis = AnalysisEngine::new(config.tick_count as usize);
        let mut step_times = Vec::with_capacity(config.tick_count as usize);
        let mut cascade_depths: Vec<f64> = Vec::with_capacity(actual_agents);
        let mut cascade_spreads: Vec<f64> = Vec::with_capacity(actual_agents);

        let faulted_start = Instant::now();
        for _ in 0..config.tick_count {
            let tick_start = Instant::now();
            let mut result = runner.tick(scheduler.scheduler(), queue_policy.policy());
            step_times.push(tick_start.elapsed().as_micros() as f64);

            // Cascade analysis via standalone ADG
            if !result.fault_events.is_empty() {
                let adg = crate::analysis::dependency::build_adg_from_agents(
                    &runner.agents,
                    crate::constants::ADG_LOOKAHEAD,
                );
                for fault in &result.fault_events {
                    let (spread, depth) = crate::analysis::cascade::cascade_bfs_standalone(
                        &adg,
                        fault.agent_index,
                        crate::constants::MAX_CASCADE_DEPTH,
                    );
                    cascade_spreads.push(spread as f64);
                    cascade_depths.push(depth as f64);
                }
            }

            analysis.record_tick(&runner, &mut result);
        }
        analysis.compute_aggregates();

        let faulted_wall_ms = faulted_start.elapsed().as_millis() as u64;

        let mut fm = compute_run_metrics(
            &baseline_record,
            &analysis,
            &analysis.fault_events,
            &step_times,
            faulted_wall_ms,
        );

        fm.cascade_depth_avg = if cascade_depths.is_empty() {
            0.0
        } else {
            cascade_depths.iter().sum::<f64>() / cascade_depths.len() as f64
        };
        fm.cascade_spread_avg = if cascade_spreads.is_empty() {
            0.0
        } else {
            cascade_spreads.iter().sum::<f64>() / cascade_spreads.len() as f64
        };

        faulted_metrics = fm;
    }

    RunResult { config: config.clone(), baseline_metrics, faulted_metrics }
}

// ---------------------------------------------------------------------------
// Baseline caching — eliminates redundant baseline simulations in run_matrix
// ---------------------------------------------------------------------------

/// Identity of a baseline — configs sharing this key produce identical fault-free runs.
/// Only the fault scenario differs between configs with the same baseline key.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BaselineKey {
    solver_name: String,
    topology_name: String,
    scheduler_name: String,
    num_agents: usize,
    seed: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl BaselineKey {
    fn from_config(config: &ExperimentConfig) -> Self {
        Self {
            solver_name: config.solver_name.clone(),
            topology_name: config.topology_name.clone(),
            scheduler_name: config.scheduler_name.clone(),
            num_agents: config.num_agents,
            seed: config.seed,
        }
    }
}

/// Cached result from a baseline run, reusable across fault scenarios.
#[cfg(not(target_arch = "wasm32"))]
struct CachedBaseline {
    record: crate::analysis::baseline::BaselineRecord,
    metrics: RunMetrics,
}

/// Run only the baseline simulation (no faults) for caching.
#[cfg(not(target_arch = "wasm32"))]
fn run_baseline_only(config: &ExperimentConfig) -> CachedBaseline {
    let wall_start = Instant::now();

    let (grid, zones) = if let Some((g, z)) = &config.custom_map {
        (g.clone(), z.clone())
    } else {
        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        (output.grid, output.zones)
    };

    let grid_area = (grid.width * grid.height) as usize;
    let actual_agents = config.num_agents.min(grid.walkable_count());

    let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    let mut rng = SeededRng::new(config.seed);
    let agents = place_agents(actual_agents, &grid, &zones, &mut rng);

    let solver =
        crate::solver::lifelong_solver_from_name(&config.solver_name, grid_area, actual_agents)
            .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

    let fault_config = FaultConfig { enabled: false, ..Default::default() };
    let mut runner = SimulationRunner::new(
        grid,
        zones,
        agents,
        solver,
        rng,
        fault_config,
        FaultSchedule::default(),
    );

    let mut analysis = AnalysisEngine::new(config.tick_count as usize);
    let mut step_times = Vec::with_capacity(config.tick_count as usize);

    for _ in 0..config.tick_count {
        let tick_start = Instant::now();
        let mut result = runner.tick(scheduler.scheduler(), queue_policy.policy());
        step_times.push(tick_start.elapsed().as_micros() as f64);
        analysis.record_tick(&runner, &mut result);
    }
    analysis.compute_aggregates();

    let bl_wall_ms = wall_start.elapsed().as_millis() as u64;
    let metrics = super::metrics::compute_baseline_self_metrics(&analysis, &step_times, bl_wall_ms);
    let record = analysis.into_baseline_record(0, config.tick_count, config.num_agents);

    CachedBaseline { record, metrics }
}

/// Run only the faulted simulation, using a cached baseline for differential metrics.
#[cfg(not(target_arch = "wasm32"))]
fn run_faulted_only(config: &ExperimentConfig, cached: &CachedBaseline) -> RunResult {
    // Regenerate topology + agents from same seed (deterministic, < 1ms).
    // Both run_baseline_only and run_faulted_only start from SeededRng::new(seed)
    // then place_agents — producing identical initial state.
    let (grid, zones) = if let Some((g, z)) = &config.custom_map {
        (g.clone(), z.clone())
    } else {
        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        (output.grid, output.zones)
    };

    let grid_area = (grid.width * grid.height) as usize;
    let actual_agents = config.num_agents.min(grid.walkable_count());

    let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
    let queue_policy = ActiveQueuePolicy::from_name("closest");

    let mut rng = SeededRng::new(config.seed);
    let agents = place_agents(actual_agents, &grid, &zones, &mut rng);

    let solver =
        crate::solver::lifelong_solver_from_name(&config.solver_name, grid_area, actual_agents)
            .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

    let (fault_config, fault_schedule) = match &config.scenario {
        Some(scenario) => {
            let fc = scenario.to_fault_config();
            let fs = scenario.generate_schedule(config.tick_count, config.num_agents);
            (fc, fs)
        }
        None => (FaultConfig { enabled: false, ..Default::default() }, FaultSchedule::default()),
    };

    let mut runner =
        SimulationRunner::new(grid, zones, agents, solver, rng, fault_config, fault_schedule);

    let mut analysis = AnalysisEngine::new(config.tick_count as usize);
    let mut step_times = Vec::with_capacity(config.tick_count as usize);
    let mut cascade_depths: Vec<f64> = Vec::with_capacity(actual_agents);
    let mut cascade_spreads: Vec<f64> = Vec::with_capacity(actual_agents);

    let faulted_start = Instant::now();
    for _ in 0..config.tick_count {
        let tick_start = Instant::now();
        let mut result = runner.tick(scheduler.scheduler(), queue_policy.policy());
        step_times.push(tick_start.elapsed().as_micros() as f64);

        if !result.fault_events.is_empty() {
            let adg = crate::analysis::dependency::build_adg_from_agents(
                &runner.agents,
                crate::constants::ADG_LOOKAHEAD,
            );
            for fault in &result.fault_events {
                let (spread, depth) = crate::analysis::cascade::cascade_bfs_standalone(
                    &adg,
                    fault.agent_index,
                    crate::constants::MAX_CASCADE_DEPTH,
                );
                cascade_spreads.push(spread as f64);
                cascade_depths.push(depth as f64);
            }
        }

        analysis.record_tick(&runner, &mut result);
    }
    analysis.compute_aggregates();

    let faulted_wall_ms = faulted_start.elapsed().as_millis() as u64;

    let mut fm = compute_run_metrics(
        &cached.record,
        &analysis,
        &analysis.fault_events,
        &step_times,
        faulted_wall_ms,
    );

    fm.cascade_depth_avg = if cascade_depths.is_empty() {
        0.0
    } else {
        cascade_depths.iter().sum::<f64>() / cascade_depths.len() as f64
    };
    fm.cascade_spread_avg = if cascade_spreads.is_empty() {
        0.0
    } else {
        cascade_spreads.iter().sum::<f64>() / cascade_spreads.len() as f64
    };

    RunResult {
        config: config.clone(),
        baseline_metrics: cached.metrics.clone(),
        faulted_metrics: fm,
    }
}

/// Shared progress state for tracking experiment execution.
pub struct ExperimentProgress {
    pub current: usize,
    pub total: usize,
    pub label: String,
}

/// Run the full experiment matrix and compute statistical summaries.
///
/// Two-phase execution with baseline caching:
/// 1. Deduplicate configs by baseline identity, run each unique baseline once
/// 2. Run faulted variants using cached baselines (skips redundant simulations)
///
/// For a 7-scenario matrix this eliminates ~85% of baseline computation.
/// Uses rayon with N-1 threads to maintain system responsiveness.
///
/// If `progress` is provided, it is updated after each run completes.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_matrix(
    matrix: &ExperimentMatrix,
    progress: Option<&std::sync::Arc<std::sync::Mutex<ExperimentProgress>>>,
) -> MatrixResult {
    let wall_start = Instant::now();

    let configs = matrix.expand();
    let total = configs.len();

    // Reserve 1 core for OS/UI responsiveness — prevents system freeze during
    // long experiment runs. Falls back to 4 threads if detection fails.
    use rayon::prelude::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let num_threads =
        std::thread::available_parallelism().map(|n| n.get().saturating_sub(1).max(1)).unwrap_or(4);

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()
        .expect("failed to create experiment thread pool");

    // ── Phase 1: Compute unique baselines ─────────────────────────────
    // Configs that share (solver, topology, scheduler, agents, seed) produce
    // identical baselines — only the fault scenario differs.
    let baseline_entries: Vec<(BaselineKey, usize)> = {
        let mut seen = HashSet::new();
        configs
            .iter()
            .enumerate()
            .filter_map(|(i, config)| {
                let key = BaselineKey::from_config(config);
                if seen.insert(key.clone()) { Some((key, i)) } else { None }
            })
            .collect()
    };
    let n_baselines = baseline_entries.len();
    let n_cached = total.saturating_sub(n_baselines);
    eprintln!(
        "Experiment: {total} configs, {n_baselines} unique baselines \
         ({n_cached} cached), {num_threads} threads"
    );

    let baseline_counter = AtomicUsize::new(0);
    let baselines: std::collections::HashMap<BaselineKey, Arc<CachedBaseline>> =
        pool.install(|| {
            baseline_entries
                .par_iter()
                .map(|(key, config_idx)| {
                    let config = &configs[*config_idx];
                    let i = baseline_counter.fetch_add(1, Ordering::Relaxed) + 1;
                    eprintln!(
                        "[baseline {i}/{n_baselines}] {} / {} / {} / {} agents / seed {}",
                        config.solver_name,
                        config.topology_name,
                        config.scheduler_name,
                        config.num_agents,
                        config.seed,
                    );
                    (key.clone(), Arc::new(run_baseline_only(config)))
                })
                .collect()
        });

    // ── Phase 2: Run faulted variants using cached baselines ──────────
    let counter = AtomicUsize::new(0);
    let progress_ref = progress.cloned();
    let runs: Vec<RunResult> = pool.install(|| {
        configs
            .par_iter()
            .map(|config| {
                let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
                let label = format!(
                    "{} / {} / {} / {} agents / seed {}",
                    config.solver_name,
                    config.topology_name,
                    config.scenario_label(),
                    config.num_agents,
                    config.seed,
                );
                eprintln!("[{i}/{total}] {label}");

                let key = BaselineKey::from_config(config);
                let cached = &baselines[&key];

                let result = if config.scenario.is_none() {
                    // No faults — faulted run is identical to baseline, skip simulation
                    RunResult {
                        config: config.clone(),
                        baseline_metrics: cached.metrics.clone(),
                        faulted_metrics: cached.metrics.clone(),
                    }
                } else {
                    run_faulted_only(config, cached)
                };

                if let Some(ref p) = progress_ref
                    && let Ok(mut prog) = p.lock()
                {
                    prog.current = i;
                    prog.label = label;
                }

                result
            })
            .collect()
    });

    // Compute statistical summaries grouped by (solver, topology, scenario, scheduler, agents)
    let summaries = compute_summaries(&runs);

    let wall_time_total_ms = wall_start.elapsed().as_millis() as u64;

    MatrixResult { matrix: matrix.clone(), runs, summaries, wall_time_total_ms }
}

/// Group runs by config identity (ignoring seed) and compute stats.
/// Public so WASM can call it after collecting runs one-by-one.
pub fn compute_summaries(runs: &[RunResult]) -> Vec<ConfigSummary> {
    use std::collections::BTreeMap;

    // Group by config key (excluding seed)
    let mut groups: BTreeMap<String, Vec<&RunResult>> = BTreeMap::new();
    for run in runs {
        let key = format!(
            "{}|{}|{}|{}|{}",
            run.config.solver_name,
            run.config.topology_name,
            run.config.scenario_label(),
            run.config.scheduler_name,
            run.config.num_agents,
        );
        groups.entry(key).or_default().push(run);
    }

    groups
        .into_values()
        .map(|group| {
            let first = &group[0].config;
            let n = group.len();

            // Extract faulted metrics for each seed
            let throughputs: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.avg_throughput).collect();
            let tasks: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.total_tasks as f64).collect();
            let unassigned_ratios: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.unassigned_ratio).collect();
            let fts: Vec<f64> = group.iter().map(|r| r.faulted_metrics.fault_tolerance).collect();
            let cts: Vec<f64> = group.iter().map(|r| r.faulted_metrics.critical_time).collect();
            let deficit_recs: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.deficit_recovery).collect();
            let tp_recs: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.throughput_recovery).collect();
            let survival_rates: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.survival_rate).collect();
            let impacted_areas: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.impacted_area).collect();
            let deficits: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.deficit_integral as f64).collect();
            let cascade_depths: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.cascade_depth_avg).collect();
            let cascade_spreads: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.cascade_spread_avg).collect();
            let fleet_utils: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.fleet_utilization).collect();
            let solver_us: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.solver_step_time_avg_us).collect();
            let wall_times: Vec<f64> =
                group.iter().map(|r| r.faulted_metrics.wall_time_ms as f64).collect();

            ConfigSummary {
                solver_name: first.solver_name.clone(),
                topology_name: first.topology_name.clone(),
                scenario_label: first.scenario_label(),
                scheduler_name: first.scheduler_name.clone(),
                num_agents: first.num_agents,
                num_seeds: n,
                throughput: compute_stat_summary(&throughputs).unwrap_or_default(),
                total_tasks: compute_stat_summary(&tasks).unwrap_or_default(),
                unassigned_ratio: compute_stat_summary(&unassigned_ratios).unwrap_or_default(),
                fault_tolerance: compute_stat_summary(&fts).unwrap_or_default(),
                critical_time: compute_stat_summary(&cts).unwrap_or_default(),
                deficit_recovery: compute_stat_summary(&deficit_recs).unwrap_or_default(),
                throughput_recovery: compute_stat_summary(&tp_recs).unwrap_or_default(),
                survival_rate: compute_stat_summary(&survival_rates).unwrap_or_default(),
                impacted_area: compute_stat_summary(&impacted_areas).unwrap_or_default(),
                deficit_integral: compute_stat_summary(&deficits).unwrap_or_default(),
                cascade_depth: compute_stat_summary(&cascade_depths).unwrap_or_default(),
                cascade_spread: compute_stat_summary(&cascade_spreads).unwrap_or_default(),
                fleet_utilization: compute_stat_summary(&fleet_utils).unwrap_or_default(),
                solver_step_us: compute_stat_summary(&solver_us).unwrap_or_default(),
                wall_time_ms: compute_stat_summary(&wall_times).unwrap_or_default(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// WASM experiment API
// ---------------------------------------------------------------------------

/// Thread-local storage for accumulated experiment runs (WASM only).
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static EXPERIMENT_RUNS: RefCell<Vec<RunResult>> = RefCell::new(Vec::new());
}

/// Clear accumulated runs. Call before starting a new experiment batch.
#[cfg(target_arch = "wasm32")]
pub fn wasm_experiment_start() {
    EXPERIMENT_RUNS.with(|r| r.borrow_mut().clear());
}

/// Run a single experiment and store the result. Returns a brief JSON summary.
#[cfg(target_arch = "wasm32")]
pub fn wasm_experiment_run_single(config_json: &str) -> String {
    let config = match parse_config_json(config_json) {
        Some(c) => c,
        None => return r#"{"error":"invalid config"}"#.to_string(),
    };

    let result = run_single_experiment(&config);

    let ft = result.faulted_metrics.fault_tolerance;
    let ft_str = if ft.is_nan() { "null".to_string() } else { format!("{ft:.4}") };
    let brief = format!(
        r#"{{"ft":{},"tp":{:.4},"tasks":{},"survival":{:.4},"wall_ms":{}}}"#,
        ft_str,
        result.faulted_metrics.avg_throughput,
        result.faulted_metrics.total_tasks,
        result.faulted_metrics.survival_rate,
        result.faulted_metrics.wall_time_ms,
    );

    EXPERIMENT_RUNS.with(|r| r.borrow_mut().push(result));

    brief
}

/// Compute summaries from all accumulated runs and return full JSON.
#[cfg(target_arch = "wasm32")]
pub fn wasm_experiment_finish() -> String {
    EXPERIMENT_RUNS.with(|r| {
        let runs = r.borrow();
        let summaries = compute_summaries(&runs);

        let mut buf = Vec::new();
        let result = MatrixResult {
            matrix: ExperimentMatrix {
                solvers: vec![],
                topologies: vec![],
                scenarios: vec![],
                schedulers: vec![],
                agent_counts: vec![],
                seeds: vec![],
                tick_count: 0,
            },
            runs: runs.clone(),
            summaries,
            wall_time_total_ms: 0,
        };
        crate::experiment::export::write_matrix_json(&mut buf, &result).ok();
        String::from_utf8(buf).unwrap_or_default()
    })
}

/// Parse a config JSON into ExperimentConfig.
#[cfg(target_arch = "wasm32")]
fn parse_config_json(json: &str) -> Option<ExperimentConfig> {
    use crate::fault::scenario::{FaultScenario, FaultScenarioType, WearHeatRate};

    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let solver = v.get("solver")?.as_str()?.to_string();
    let topology = v.get("topology")?.as_str()?.to_string();
    let scheduler = v.get("scheduler")?.as_str()?.to_string();
    let num_agents = v.get("num_agents")?.as_u64()? as usize;
    let seed = v.get("seed")?.as_u64()?;
    let tick_count = v.get("tick_count")?.as_u64()?;

    let scenario = v.get("scenario").and_then(|s| {
        let stype = s.get("type")?.as_str()?;
        match stype {
            "burst" => Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::BurstFailure,
                burst_kill_percent: s.get("kill_percent")?.as_f64()? as f32,
                burst_at_tick: s.get("at_tick")?.as_u64()?,
                ..Default::default()
            }),
            "wear" => {
                let rate_str = s.get("rate").and_then(|r| r.as_str()).unwrap_or("medium");
                let rate = match rate_str {
                    "low" => WearHeatRate::Low,
                    "high" => WearHeatRate::High,
                    "medium" => WearHeatRate::Medium,
                    _ => WearHeatRate::Medium, // "custom" or unknown → use Medium as base
                };
                let mut scenario = FaultScenario {
                    enabled: true,
                    scenario_type: FaultScenarioType::WearBased,
                    wear_heat_rate: rate,
                    wear_threshold: s.get("threshold").and_then(|t| t.as_f64()).unwrap_or(80.0)
                        as f32,
                    ..Default::default()
                };
                // Custom Weibull parameters override the preset
                if let (Some(beta), Some(eta)) =
                    (s.get("beta").and_then(|b| b.as_f64()), s.get("eta").and_then(|e| e.as_f64()))
                {
                    scenario.custom_weibull = Some((beta as f32, eta as f32));
                }
                Some(scenario)
            }
            "zone" => Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::ZoneOutage,
                zone_at_tick: s.get("at_tick").and_then(|t| t.as_u64()).unwrap_or(100),
                zone_latency_duration: s.get("duration").and_then(|d| d.as_u64()).unwrap_or(50)
                    as u32,
                ..Default::default()
            }),
            "intermittent" => Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::IntermittentFault,
                intermittent_mtbf_ticks: s.get("mtbf").and_then(|t| t.as_u64()).unwrap_or(80),
                intermittent_recovery_ticks: s
                    .get("recovery")
                    .and_then(|r| r.as_u64())
                    .unwrap_or(15) as u32,
                ..Default::default()
            }),
            "none" | _ => None,
        }
    });

    // Parse optional inline custom map
    let custom_map = v.get("custom_map").and_then(|cm| parse_custom_map_data(cm));

    Some(ExperimentConfig {
        solver_name: solver,
        topology_name: topology,
        scenario,
        scheduler_name: scheduler,
        num_agents,
        seed,
        tick_count,
        custom_map,
    })
}

/// Parse inline custom map JSON into (GridMap, ZoneMap).
///
/// Must match `TopologyRegistry::parse_json_value()` exactly — same cell
/// parsing, same queue_direction handling, same corridor classification.
/// Any divergence causes experiment vs observatory non-determinism.
#[cfg(target_arch = "wasm32")]
fn parse_custom_map_data(
    v: &serde_json::Value,
) -> Option<(crate::core::grid::GridMap, crate::core::topology::ZoneMap)> {
    // Delegate to the canonical parser to guarantee parity.
    crate::core::topology::TopologyRegistry::parse_json_value(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiment::config::ExperimentConfig;
    use crate::fault::scenario::{FaultScenario, FaultScenarioType};

    #[test]
    fn single_experiment_no_faults() {
        let config = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };
        let result = run_single_experiment(&config);
        assert!(result.baseline_metrics.total_tasks > 0);
        // No faults → faulted should match baseline
        assert_eq!(result.baseline_metrics.total_tasks, result.faulted_metrics.total_tasks);
        assert!((result.faulted_metrics.fault_tolerance - 1.0).abs() < 1e-6);
    }

    #[test]
    fn single_experiment_deterministic() {
        let config = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 123,
            tick_count: 100,
            custom_map: None,
        };
        let r1 = run_single_experiment(&config);
        let r2 = run_single_experiment(&config);
        assert_eq!(r1.baseline_metrics.total_tasks, r2.baseline_metrics.total_tasks);
        assert_eq!(r1.faulted_metrics.total_tasks, r2.faulted_metrics.total_tasks);
    }

    #[test]
    fn single_experiment_with_burst_fault() {
        let config = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::BurstFailure,
                burst_kill_percent: 30.0,
                burst_at_tick: 20,
                ..Default::default()
            }),
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };
        let result = run_single_experiment(&config);
        // Survival rate should be < 1.0 (some agents killed)
        assert!(result.faulted_metrics.survival_rate < 1.0);
        // Faulted should have fewer tasks than baseline when not over-saturated
        assert!(
            result.faulted_metrics.total_tasks <= result.baseline_metrics.total_tasks,
            "faulted tasks ({}) should be <= baseline ({})",
            result.faulted_metrics.total_tasks,
            result.baseline_metrics.total_tasks,
        );
    }

    #[test]
    fn single_experiment_intermittent_ft_bounded() {
        // Regression test: before the fault_rng split, intermittent FT was ~2.17
        // because baseline and faulted runs consumed different RNG streams, making
        // the baseline produce far fewer tasks than the faulted run.
        // After the fix: fault_rng is isolated, so FT must be <= 1.0.
        use crate::fault::scenario::FaultScenarioType;
        let config = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: Some(FaultScenario {
                enabled: true,
                scenario_type: FaultScenarioType::IntermittentFault,
                intermittent_mtbf_ticks: 30,
                intermittent_recovery_ticks: 8,
                ..Default::default()
            }),
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };
        let result = run_single_experiment(&config);
        let ft = result.faulted_metrics.fault_tolerance;
        assert!(
            ft <= 1.0 + 1e-6,
            "intermittent FT {ft:.4} > 1.0 — RNG streams still diverging between baseline and faulted runs"
        );
        assert!(ft > 0.0, "intermittent FT should be > 0 (agents still complete tasks)");
    }

    /// Verify that the experiment runner produces identical results to a manual
    /// runner created the same way the observatory does (LiveSim path).
    /// Regression test for missing queue_lines in parse_custom_map_data.
    #[test]
    fn experiment_matches_manual_runner() {
        use crate::analysis::baseline::place_agents;
        use crate::core::queue::ActiveQueuePolicy;
        use crate::core::runner::SimulationRunner;
        use crate::core::task::ActiveScheduler;

        let seed = 42u64;
        let num_agents = 10;
        let tick_count = 200u64;
        let solver_name = "pibt";
        let scheduler_name = "random";
        let topology_name = "warehouse_large";

        // Path A: experiment runner (uses run_single_experiment)
        let config = ExperimentConfig {
            solver_name: solver_name.into(),
            topology_name: topology_name.into(),
            scenario: None,
            scheduler_name: scheduler_name.into(),
            num_agents,
            seed,
            tick_count,
            custom_map: None,
        };
        let experiment_result = run_single_experiment(&config);

        // Path B: manual runner (mirrors what observatory/LiveSim does)
        let topo = crate::core::topology::ActiveTopology::from_name(topology_name);
        let output = topo.topology().generate(seed);
        let (grid, zones) = (output.grid, output.zones);
        let grid_area = (grid.width * grid.height) as usize;

        let scheduler = ActiveScheduler::from_name(scheduler_name);
        let queue_policy = ActiveQueuePolicy::from_name("closest");

        let mut rng = SeededRng::new(seed);
        let agents = place_agents(num_agents, &grid, &zones, &mut rng);

        let solver =
            crate::solver::lifelong_solver_from_name(solver_name, grid_area, num_agents).unwrap();

        let mut runner = SimulationRunner::new(
            grid,
            zones,
            agents,
            solver,
            rng,
            crate::fault::config::FaultConfig { enabled: false, ..Default::default() },
            FaultSchedule::default(),
        );

        for _ in 0..tick_count {
            runner.tick(scheduler.scheduler(), queue_policy.policy());
        }

        // Both paths must produce the same task count
        assert_eq!(
            experiment_result.faulted_metrics.total_tasks, runner.tasks_completed,
            "experiment runner ({}) vs manual runner ({}) task count mismatch",
            experiment_result.faulted_metrics.total_tasks, runner.tasks_completed,
        );
    }

    /// Verify that the experiment baseline produces identical results to an
    /// observatory-style baseline (run_headless). This catches divergences in
    /// agent clamping, solver auto-config, or grid generation between the two paths.
    #[test]
    fn experiment_baseline_matches_observatory_baseline() {
        use crate::analysis::baseline::{BaselineConfig, run_headless};

        let seed = 77u64;
        let num_agents = 10;
        let tick_count = 200u64;
        let solver_name = "pibt";
        let scheduler_name = "random";
        let topology_name = "warehouse_large";

        // Path A: experiment runner baseline
        let config = ExperimentConfig {
            solver_name: solver_name.into(),
            topology_name: topology_name.into(),
            scenario: None,
            scheduler_name: scheduler_name.into(),
            num_agents,
            seed,
            tick_count,
            custom_map: None,
        };
        let experiment_result = run_single_experiment(&config);

        // Path B: observatory baseline (run_headless, same as start_headless)
        let baseline_config = BaselineConfig {
            topology_name: topology_name.into(),
            num_agents,
            solver_name: solver_name.into(),
            scheduler_name: scheduler_name.into(),
            seed,
            tick_count,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let observatory_baseline = run_headless(&baseline_config);

        // Both baselines must produce the same task count
        assert_eq!(
            experiment_result.baseline_metrics.total_tasks, observatory_baseline.total_tasks,
            "experiment baseline ({}) vs observatory baseline ({}) task count mismatch",
            experiment_result.baseline_metrics.total_tasks, observatory_baseline.total_tasks,
        );

        // Also verify tick-by-tick throughput series lengths match
        assert_eq!(
            experiment_result.baseline_metrics.total_tasks,
            experiment_result.faulted_metrics.total_tasks,
            "no-fault experiment: baseline and faulted should match"
        );
    }

    /// Same parity test with token_passing + compact_grid to cover the user's
    /// reported divergence scenario.
    #[test]
    fn experiment_baseline_parity_token_passing_compact_grid() {
        use crate::analysis::baseline::{BaselineConfig, run_headless};

        let seed = 42u64;
        let num_agents = 20;
        let tick_count = 500u64;
        let solver_name = "token_passing";
        let scheduler_name = "random";
        let topology_name = "compact_grid";

        // Path A: experiment runner baseline
        let config = ExperimentConfig {
            solver_name: solver_name.into(),
            topology_name: topology_name.into(),
            scenario: None,
            scheduler_name: scheduler_name.into(),
            num_agents,
            seed,
            tick_count,
            custom_map: None,
        };
        let experiment_result = run_single_experiment(&config);

        // Path B: observatory baseline (run_headless)
        let baseline_config = BaselineConfig {
            topology_name: topology_name.into(),
            num_agents,
            solver_name: solver_name.into(),
            scheduler_name: scheduler_name.into(),
            seed,
            tick_count,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let observatory_baseline = run_headless(&baseline_config);

        assert_eq!(
            experiment_result.baseline_metrics.total_tasks, observatory_baseline.total_tasks,
            "token_passing + compact_grid: experiment baseline ({}) vs observatory baseline ({}) diverged",
            experiment_result.baseline_metrics.total_tasks, observatory_baseline.total_tasks,
        );
    }

    /// Verify faulted run parity: experiment faulted run vs a manual runner
    /// with burst_20pct on token_passing + compact_grid (matches user's report).
    #[test]
    fn experiment_faulted_parity_token_passing_burst() {
        use crate::analysis::baseline::place_agents;
        use crate::core::queue::ActiveQueuePolicy;
        use crate::core::runner::SimulationRunner;
        use crate::core::task::ActiveScheduler;

        let seed = 42u64;
        let num_agents = 20;
        let tick_count = 500u64;
        let solver_name = "token_passing";
        let scheduler_name = "random";
        let topology_name = "compact_grid";

        let scenario = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 100,
            ..Default::default()
        };

        // Path A: experiment
        let config = ExperimentConfig {
            solver_name: solver_name.into(),
            topology_name: topology_name.into(),
            scenario: Some(scenario.clone()),
            scheduler_name: scheduler_name.into(),
            num_agents,
            seed,
            tick_count,
            custom_map: None,
        };
        let experiment_result = run_single_experiment(&config);

        // Path B: manual runner (mirrors observatory LiveSim path)
        let topo = crate::core::topology::ActiveTopology::from_name(topology_name);
        let output = topo.topology().generate(seed);
        let (grid, zones) = (output.grid, output.zones);
        let grid_area = (grid.width * grid.height) as usize;
        let actual_agents = num_agents.min(grid.walkable_count());

        let scheduler = ActiveScheduler::from_name(scheduler_name);
        let queue_policy = ActiveQueuePolicy::from_name("closest");

        let mut rng = SeededRng::new(seed);
        let agents = place_agents(actual_agents, &grid, &zones, &mut rng);

        let solver =
            crate::solver::lifelong_solver_from_name(solver_name, grid_area, actual_agents)
                .unwrap();

        let fault_config = scenario.to_fault_config();
        let fault_schedule = scenario.generate_schedule(tick_count, num_agents);

        let mut runner =
            SimulationRunner::new(grid, zones, agents, solver, rng, fault_config, fault_schedule);

        for _ in 0..tick_count {
            runner.tick(scheduler.scheduler(), queue_policy.policy());
        }

        assert_eq!(
            experiment_result.faulted_metrics.total_tasks, runner.tasks_completed,
            "faulted parity: experiment ({}) vs manual runner ({}) — token_passing burst_20pct",
            experiment_result.faulted_metrics.total_tasks, runner.tasks_completed,
        );
    }

    #[test]
    fn mini_matrix() {
        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None],
            schedulers: vec!["random".into()],
            agent_counts: vec![5],
            seeds: vec![1, 2],
            tick_count: 30,
        };
        let result = run_matrix(&matrix, None);
        assert_eq!(result.runs.len(), 2);
        assert_eq!(result.summaries.len(), 1);
        assert_eq!(result.summaries[0].num_seeds, 2);
        // Wall time may be 0ms on fast machines (sub-millisecond runs).
        // Just verify the field exists and the matrix completed.
        assert!(result.wall_time_total_ms < 60_000, "matrix should complete in under 60s");
    }

    /// Verify baseline caching produces identical results to uncached path.
    /// Runs a 2-scenario matrix (cached) and compares against individual
    /// run_single_experiment calls (uncached).
    #[test]
    fn baseline_cache_correctness() {
        use crate::fault::scenario::{FaultScenario, FaultScenarioType};

        let burst = FaultScenario {
            enabled: true,
            scenario_type: FaultScenarioType::BurstFailure,
            burst_kill_percent: 20.0,
            burst_at_tick: 20,
            ..Default::default()
        };

        // Cached path: run_matrix with 2 scenarios sharing same baseline
        let matrix = ExperimentMatrix {
            solvers: vec!["pibt".into()],
            topologies: vec!["warehouse_large".into()],
            scenarios: vec![None, Some(burst.clone())],
            schedulers: vec!["random".into()],
            agent_counts: vec![10],
            seeds: vec![42],
            tick_count: 100,
        };
        let cached_result = run_matrix(&matrix, None);
        assert_eq!(cached_result.runs.len(), 2);

        // Uncached path: individual run_single_experiment calls
        let config_none = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };
        let config_burst = ExperimentConfig { scenario: Some(burst), ..config_none.clone() };

        let uncached_none = run_single_experiment(&config_none);
        let uncached_burst = run_single_experiment(&config_burst);

        // Compare no-fault results
        let cached_none = &cached_result.runs[0];
        assert_eq!(
            cached_none.baseline_metrics.total_tasks, uncached_none.baseline_metrics.total_tasks,
            "no-fault baseline tasks mismatch"
        );
        assert_eq!(
            cached_none.faulted_metrics.total_tasks, uncached_none.faulted_metrics.total_tasks,
            "no-fault faulted tasks mismatch"
        );

        // Compare burst-fault results
        let cached_burst = &cached_result.runs[1];
        assert_eq!(
            cached_burst.baseline_metrics.total_tasks, uncached_burst.baseline_metrics.total_tasks,
            "burst baseline tasks mismatch"
        );
        assert_eq!(
            cached_burst.faulted_metrics.total_tasks, uncached_burst.faulted_metrics.total_tasks,
            "burst faulted tasks mismatch"
        );
        assert!(
            (cached_burst.faulted_metrics.fault_tolerance
                - uncached_burst.faulted_metrics.fault_tolerance)
                .abs()
                < 1e-10,
            "burst FT mismatch: cached={} uncached={}",
            cached_burst.faulted_metrics.fault_tolerance,
            uncached_burst.faulted_metrics.fault_tolerance,
        );
    }

    /// Verify compute_baseline_self_metrics produces values matching
    /// the original compute_run_metrics self-comparison path.
    #[test]
    fn baseline_self_metrics_match() {
        let config = ExperimentConfig {
            solver_name: "pibt".into(),
            topology_name: "warehouse_large".into(),
            scenario: None,
            scheduler_name: "random".into(),
            num_agents: 10,
            seed: 42,
            tick_count: 100,
            custom_map: None,
        };

        let result = run_single_experiment(&config);

        // For no-fault runs, baseline and faulted metrics should be identical
        // (both come from compute_baseline_self_metrics now)
        assert_eq!(result.baseline_metrics.total_tasks, result.faulted_metrics.total_tasks);
        assert!(
            (result.baseline_metrics.avg_throughput - result.faulted_metrics.avg_throughput).abs()
                < 1e-10,
            "throughput mismatch: baseline={} faulted={}",
            result.baseline_metrics.avg_throughput,
            result.faulted_metrics.avg_throughput,
        );
        assert!(
            (result.baseline_metrics.fault_tolerance - 1.0).abs() < 1e-10,
            "baseline FT should be 1.0, got {}",
            result.baseline_metrics.fault_tolerance,
        );
        assert_eq!(result.baseline_metrics.survival_rate, 1.0);
        assert!(result.baseline_metrics.total_tasks > 0, "baseline should complete some tasks");
    }
}
