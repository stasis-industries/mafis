//! Headless baseline simulation engine.
//!
//! Runs a fault-free simulation without Bevy ECS — pure Rust structs and loops.
//! Produces a `BaselineRecord` that the differential dashboard compares against
//! the live fault simulation.
//!
//! The headless loop replicates `tick_agents → recycle_goals → lifelong_replan`
//! faithfully: same collision resolution, same task scheduling, same solver.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::analysis::TimeSeriesAccessor;
use crate::core::grid::GridMap;
use crate::core::placement::{find_from_pool, find_random_walkable};
use crate::core::queue::ActiveQueuePolicy;
use crate::core::runner::{SimAgent, SimulationRunner};
use crate::core::seed::SeededRng;
use crate::core::task::ActiveScheduler;
use crate::core::topology::{ActiveTopology, ZoneMap};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BaselineConfig {
    pub topology_name: String,
    pub num_agents: usize,
    pub solver_name: String,
    pub scheduler_name: String,
    pub seed: u64,
    pub tick_count: u64,
    /// Pre-built grid+zones for custom topologies. When `Some`, `run_headless`
    /// uses these directly instead of regenerating from `topology_name`.
    pub grid_override: Option<(GridMap, ZoneMap)>,
    /// When true, the headless loop consumes one RNG draw per agent per tick
    /// to match the live `detect_faults` system's deterministic RNG consumption.
    /// Without this, the RNG stream diverges between headless and live paths.
    pub fault_enabled: bool,
    /// Pre-specified agent start positions from an imported scenario.
    /// When `Some`, `place_agents` is skipped and these positions are used instead.
    pub agent_positions: Option<Vec<(IVec2, IVec2)>>,
}

impl BaselineConfig {
    /// Deterministic hash for matching baseline to live runs.
    pub fn config_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.topology_name.hash(&mut h);
        self.num_agents.hash(&mut h);
        self.solver_name.hash(&mut h);
        self.scheduler_name.hash(&mut h);
        self.seed.hash(&mut h);
        self.tick_count.hash(&mut h);
        h.finish()
    }
}

// ---------------------------------------------------------------------------
// Record — output of a headless run
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BaselineRecord {
    pub config_hash: u64,
    pub tick_count: u64,
    pub num_agents: usize,

    // Per-tick series (indexed by tick)
    /// Instantaneous task completions per tick.
    pub throughput_series: Vec<f64>,
    pub tasks_completed_series: Vec<u64>,
    pub idle_count_series: Vec<usize>,
    /// Cumulative wait ratio (wait_actions / total_actions) at each tick.
    /// Equivalent to FaultMetrics.idle_ratio from the live ECS path.
    pub wait_ratio_series: Vec<f32>,

    // Aggregate
    pub total_tasks: u64,
    pub avg_throughput: f64,

    // Spatial
    pub traffic_counts: HashMap<IVec2, u32>,

    /// Per-tick agent positions for parity verification.
    /// `position_snapshots[t][i]` = position of agent i at effective_tick t+1.
    pub position_snapshots: Vec<Vec<IVec2>>,
}

// ---------------------------------------------------------------------------
// TimeSeriesAccessor impl
// ---------------------------------------------------------------------------

impl TimeSeriesAccessor for BaselineRecord {
    fn throughput_series(&self) -> &[f64] {
        &self.throughput_series
    }
    fn tasks_completed_series(&self) -> &[u64] {
        &self.tasks_completed_series
    }
    fn idle_count_series(&self) -> &[usize] {
        &self.idle_count_series
    }
    fn wait_ratio_series(&self) -> &[f32] {
        &self.wait_ratio_series
    }
    fn position_snapshots(&self) -> &[Vec<IVec2>] {
        &self.position_snapshots
    }
}

// ---------------------------------------------------------------------------
// Resource — stores the most recent baseline for differential comparison
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct BaselineStore {
    pub record: Option<BaselineRecord>,
    /// True while baseline is being computed (for UI progress indicator).
    pub computing: bool,
}

// ---------------------------------------------------------------------------
// BaselineDiff — per-tick differential between baseline and live simulation
// ---------------------------------------------------------------------------

/// Computed every tick in `AnalysisSet::Metrics`. Compares the deterministic
/// fault-free baseline against the live (possibly fault-injected) simulation.
#[derive(Resource, Debug, Clone)]
pub struct BaselineDiff {
    /// `baseline_tasks - live_tasks` at current tick. Positive = deficit.
    pub gap: i64,
    /// Sum of `max(0, gap)` over all ticks — total agent-ticks behind.
    pub deficit_integral: i64,
    /// Sum of `max(0, -gap)` over all ticks — total agent-ticks ahead (Braess's).
    pub surplus_integral: i64,
    /// `deficit_integral - surplus_integral` — single comparable number.
    pub net_integral: i64,
    /// `((live_tasks - baseline_tasks) / baseline_tasks) × 100` — normalized %.
    /// Negative = behind baseline. Positive = ahead (Braess's). NaN-safe.
    pub impacted_area: f64,
    /// `baseline_throughput[T] - live_throughput[T]` at current tick.
    pub rate_delta: f64,
    /// First tick where `gap <= 0` after having been `> 0`.
    pub recovery_tick: Option<u64>,
    /// Number of ticks where `gap[T] > gap[T-1]` (live falling further behind).
    pub ticks_gap_growing: u64,
    /// Tick at which first fault caused gap > 0 (for recoverability denominator).
    pub first_gap_tick: Option<u64>,
    /// Previous tick's gap value (for gap-growth detection).
    prev_gap: i64,
    /// Whether gap has ever been positive (for recovery detection).
    was_positive: bool,
}

impl Default for BaselineDiff {
    fn default() -> Self {
        Self {
            gap: 0,
            deficit_integral: 0,
            surplus_integral: 0,
            net_integral: 0,
            impacted_area: 0.0,
            rate_delta: 0.0,
            recovery_tick: None,
            ticks_gap_growing: 0,
            first_gap_tick: None,
            prev_gap: 0,
            was_positive: false,
        }
    }
}

impl BaselineDiff {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Update the diff for the current tick. Called once per FixedUpdate tick.
    pub fn update(
        &mut self,
        tick: u64,
        baseline_tasks: u64,
        live_tasks: u64,
        baseline_tp: f64,
        live_tp: f64,
    ) {
        self.gap = baseline_tasks as i64 - live_tasks as i64;
        self.rate_delta = baseline_tp - live_tp;

        // Impacted Area: ((live - baseline) / baseline) × 100
        self.impacted_area = if baseline_tasks > 0 {
            ((live_tasks as f64 - baseline_tasks as f64) / baseline_tasks as f64) * 100.0
        } else {
            0.0
        };

        // Accumulate integrals
        if self.gap > 0 {
            self.deficit_integral += self.gap;
        } else if self.gap < 0 {
            self.surplus_integral += -self.gap;
        }
        self.net_integral = self.deficit_integral - self.surplus_integral;

        // Track gap growth (for recoverability)
        if self.gap > self.prev_gap {
            self.ticks_gap_growing += 1;
        }

        // Track first time gap goes positive
        if self.gap > 0 && self.first_gap_tick.is_none() {
            self.first_gap_tick = Some(tick);
        }

        // Recovery detection: first tick where gap <= 0 after being > 0
        if self.gap > 0 {
            self.was_positive = true;
            self.recovery_tick = None; // reset — gap still positive
        } else if self.was_positive && self.recovery_tick.is_none() {
            self.recovery_tick = Some(tick);
        }

        self.prev_gap = self.gap;
    }

    /// Recompute from scratch for a range of ticks (used after rewind).
    pub fn recompute(
        &mut self,
        baseline_record: &BaselineRecord,
        live_tasks_series: &[u64],
        live_tp_series: &[f64],
    ) {
        self.clear();
        let len = live_tasks_series
            .len()
            .min(live_tp_series.len())
            .min(baseline_record.throughput_series.len());
        for i in 0..len {
            let tick = i as u64 + 1; // effective_tick = index + 1
            let bl_tasks = baseline_record.tasks_at(tick);
            let bl_tp = baseline_record.throughput_at(tick);
            self.update(tick, bl_tasks, live_tasks_series[i], bl_tp, live_tp_series[i]);
        }
    }
}

// ---------------------------------------------------------------------------
// ECS system — position parity check (runs before fault injection)
// ---------------------------------------------------------------------------

/// Compares live agent positions against baseline snapshot each tick.
/// Logs the FIRST tick where positions diverge. This catches simulation
/// parity bugs at the root (positions) rather than derived metrics (throughput).
/// Only checks ticks before any fault fires (pre-fault zone).
#[cfg(target_arch = "wasm32")]
pub fn check_position_parity(
    store: Res<BaselineStore>,
    config: Res<crate::core::state::SimulationConfig>,
    agents: Query<
        (&crate::core::agent::LogicalAgent, &crate::core::agent::AgentIndex),
        Without<crate::fault::breakdown::Dead>,
    >,
    mut logged_divergence: Local<bool>,
) {
    if *logged_divergence {
        return;
    }
    let Some(ref record) = store.record else { return };
    let Some(bl_positions) = record.positions_at(config.tick) else { return };

    // Build live positions in deterministic order
    let mut live_order: Vec<(usize, IVec2)> =
        agents.iter().map(|(a, idx)| (idx.0, a.current_pos)).collect();
    live_order.sort_unstable_by_key(|&(idx, _)| idx);

    if live_order.len() != bl_positions.len() {
        return; // agent count mismatch (fault killed agents)
    }

    for (i, (_, live_pos)) in live_order.iter().enumerate() {
        if *live_pos != bl_positions[i] {
            *logged_divergence = true;
            let msg = format!(
                "[PARITY DIVERGENCE t={}] agent {} baseline=({},{}) live=({},{}) — simulation paths split here",
                config.tick, i, bl_positions[i].x, bl_positions[i].y, live_pos.x, live_pos.y,
            );
            web_sys::console::warn_1(&msg.into());
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// ECS system — update BaselineDiff each tick
// ---------------------------------------------------------------------------

/// Computes the per-tick differential between baseline and live simulation.
/// Runs in `AnalysisSet::Metrics` during `SimState::Running`.
pub fn update_baseline_diff(
    store: Res<BaselineStore>,
    config: Res<crate::core::state::SimulationConfig>,
    lifelong: Res<crate::core::task::LifelongConfig>,
    mut diff: ResMut<BaselineDiff>,
) {
    let Some(ref record) = store.record else { return };
    let tick = config.tick;
    // Accessors accept 1-based tick and internally convert to 0-based index.
    let bl_tasks = record.tasks_at(tick);
    let bl_tp = record.throughput_at(tick);
    let live_tasks = lifelong.tasks_completed;
    let live_tp = lifelong.throughput(tick);
    diff.update(tick, bl_tasks, live_tasks, bl_tp, live_tp);
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// In-progress baseline computation. Holds all state needed to advance the
/// headless simulation across multiple frames, keeping the UI responsive.
#[derive(Resource)]
pub struct BaselineComputation {
    runner: SimulationRunner,
    analysis: super::engine::AnalysisEngine,
    scheduler: ActiveScheduler,
    queue_policy: ActiveQueuePolicy,
    pub total_ticks: u64,
    pub ticks_done: u64,
    config_hash: u64,
    num_agents: usize,
}

impl BaselineComputation {
    /// Advance the baseline by up to `batch_size` ticks. Returns true when done.
    pub fn tick_batch(&mut self, batch_size: u64) -> bool {
        let remaining = self.total_ticks - self.ticks_done;
        let to_do = remaining.min(batch_size);
        for _ in 0..to_do {
            let mut result =
                self.runner.tick(self.scheduler.scheduler(), self.queue_policy.policy());
            self.analysis.record_tick(&self.runner, &mut result);
            self.ticks_done += 1;
        }
        self.ticks_done >= self.total_ticks
    }

    /// Extract the BaselineRecord, replacing internal state with empty.
    /// Use this when you can't take ownership (e.g. from a `ResMut`).
    pub fn take_record(&mut self) -> BaselineRecord {
        self.analysis.compute_aggregates();
        debug_assert!(
            self.total_ticks == 0 || !self.analysis.throughput_series.is_empty(),
            "Baseline produced no data for {} ticks",
            self.total_ticks
        );
        let analysis = std::mem::replace(&mut self.analysis, super::engine::AnalysisEngine::new(0));
        analysis.into_baseline_record(self.config_hash, self.total_ticks, self.num_agents)
    }

    /// Finalize and return the BaselineRecord (consuming self).
    pub fn finalize(mut self) -> BaselineRecord {
        self.take_record()
    }
}

/// Set up a baseline computation without running it. Returns a
/// `BaselineComputation` that can be advanced incrementally.
pub fn start_headless(config: &BaselineConfig) -> BaselineComputation {
    // 1. Use pre-built grid/zones if provided, otherwise generate from topology
    let (grid, zones) = if let Some((ref g, ref z)) = config.grid_override {
        (g.clone(), z.clone())
    } else {
        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        (output.grid, output.zones)
    };

    // 2. Clamp agent count to walkable capacity (matches experiment runner)
    let actual_agents = config.num_agents.min(grid.walkable_count());

    // 3. Create solver + scheduler + rng
    let solver = crate::solver::lifelong_solver_from_name(
        &config.solver_name,
        (grid.width * grid.height) as usize,
        actual_agents,
    )
    .unwrap_or_else(|| Box::new(crate::solver::pibt::PibtLifelongSolver::new()));

    let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
    let queue_policy = ActiveQueuePolicy::from_name("closest");
    let mut rng = SeededRng::new(config.seed);

    // 4. Place agents (treat empty positions as None — simulateIn3D strips robots)
    let agents = if let Some(ref positions) = config.agent_positions {
        if !positions.is_empty() {
            positions.iter().map(|&(start, _goal)| SimAgent::new(start)).collect::<Vec<_>>()
        } else {
            place_agents(actual_agents, &grid, &zones, &mut rng)
        }
    } else {
        place_agents(actual_agents, &grid, &zones, &mut rng)
    };
    let n = agents.len();

    // 5. Create runner (no faults)
    let fault_config = crate::fault::config::FaultConfig { enabled: false, ..Default::default() };
    let runner = SimulationRunner::new(
        grid,
        zones,
        agents,
        solver,
        rng,
        fault_config,
        crate::fault::scenario::FaultSchedule::default(),
    );

    // 6. Create analysis engine (positions disabled — saves ~500 Vec<IVec2> allocs per run)
    let analysis = super::engine::AnalysisEngine::new(config.tick_count as usize);
    // record_positions defaults to false — headless baseline doesn't need per-tick
    // position snapshots. Parity verification runs separately in the live ECS path.

    BaselineComputation {
        runner,
        analysis,
        scheduler,
        queue_policy,
        total_ticks: config.tick_count,
        ticks_done: 0,
        config_hash: config.config_hash(),
        num_agents: n,
    }
}

/// Run a complete headless simulation and return the baseline record.
///
/// Synchronous convenience wrapper around `start_headless` + `tick_batch`.
/// Used by tests and non-WASM contexts where blocking is acceptable.
pub fn run_headless(config: &BaselineConfig) -> BaselineRecord {
    let mut comp = start_headless(config);
    comp.tick_batch(config.tick_count); // run all ticks at once
    let record = comp.finalize();
    debug_assert!(
        config.tick_count == 0 || !record.throughput_series.is_empty(),
        "Baseline produced no data for {} ticks",
        config.tick_count
    );
    record
}

// ---------------------------------------------------------------------------
// Agent placement — uses shared functions from core::placement
// ---------------------------------------------------------------------------

pub fn place_agents(
    num_agents: usize,
    grid: &GridMap,
    zones: &ZoneMap,
    rng: &mut SeededRng,
) -> Vec<SimAgent> {
    let mut used = HashSet::new();
    let mut agents = Vec::with_capacity(num_agents);

    // Prefer corridor cells for starts
    let start_pool: &[IVec2] = if !zones.corridor_cells.is_empty() {
        &zones.corridor_cells
    } else if !zones.pickup_cells.is_empty() {
        &zones.pickup_cells
    } else {
        &[]
    };

    for _ in 0..num_agents {
        let start = if !start_pool.is_empty() {
            find_from_pool(start_pool, &mut rng.rng, &used)
                .unwrap_or_else(|| find_random_walkable(grid, &mut rng.rng, &used))
        } else {
            find_random_walkable(grid, &mut rng.rng, &used)
        };
        used.insert(start);

        agents.push(SimAgent::new(start));
    }
    agents
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── BaselineDiff unit tests ────────────────────────────────────────────

    #[test]
    fn baseline_diff_zero_before_faults() {
        let mut diff = super::BaselineDiff::default();
        // Simulate 10 ticks where baseline and live are identical
        for tick in 1..=10 {
            diff.update(tick, tick * 2, tick * 2, 1.0, 1.0);
        }
        assert_eq!(diff.gap, 0);
        assert_eq!(diff.deficit_integral, 0);
        assert_eq!(diff.surplus_integral, 0);
        assert_eq!(diff.net_integral, 0);
        assert_eq!(diff.rate_delta, 0.0);
        assert!(diff.recovery_tick.is_none());
        assert_eq!(diff.ticks_gap_growing, 0);
    }

    #[test]
    fn baseline_diff_deficit_after_fault() {
        let mut diff = super::BaselineDiff::default();
        // Tick 1-5: identical (baseline=live tasks)
        for tick in 1..=5 {
            diff.update(tick, tick, tick, 1.0, 1.0);
        }
        assert_eq!(diff.gap, 0);
        // Tick 6-10: fault kills agent, live falls behind
        for tick in 6..=10 {
            diff.update(tick, tick, tick - 2, 1.0, 0.5);
        }
        assert_eq!(diff.gap, 2); // baseline_tasks(10) - live_tasks(8) = 2
        assert!(diff.deficit_integral > 0);
        assert_eq!(diff.surplus_integral, 0);
        assert!(diff.first_gap_tick == Some(6));
        assert!(diff.recovery_tick.is_none()); // still behind
    }

    #[test]
    fn baseline_diff_recovery_detected() {
        let mut diff = super::BaselineDiff::default();
        // Tick 1-3: identical
        for tick in 1..=3 {
            diff.update(tick, tick, tick, 1.0, 1.0);
        }
        // Tick 4-5: live falls behind
        diff.update(4, 4, 3, 1.0, 0.0);
        diff.update(5, 5, 4, 1.0, 1.0);
        assert!(diff.was_positive);
        assert!(diff.recovery_tick.is_none()); // gap=1, still positive
        // Tick 6: live catches up
        diff.update(6, 6, 6, 1.0, 2.0);
        assert_eq!(diff.gap, 0);
        assert_eq!(diff.recovery_tick, Some(6));
    }

    #[test]
    fn baseline_diff_surplus_braess() {
        let mut diff = super::BaselineDiff::default();
        // Live outperforms baseline (Braess's Paradox)
        for tick in 1..=5 {
            diff.update(tick, tick, tick + 1, 1.0, 2.0);
        }
        assert!(diff.gap < 0); // live ahead
        assert_eq!(diff.deficit_integral, 0);
        assert!(diff.surplus_integral > 0);
        assert!(diff.net_integral < 0);
    }

    #[test]
    fn baseline_diff_clear_resets() {
        let mut diff = super::BaselineDiff::default();
        diff.update(1, 10, 5, 1.0, 0.5);
        assert!(diff.gap > 0);
        diff.clear();
        assert_eq!(diff.gap, 0);
        assert_eq!(diff.deficit_integral, 0);
        assert_eq!(diff.surplus_integral, 0);
        assert!(diff.recovery_tick.is_none());
    }

    // ── Headless baseline tests ─────────────────────────────────────────────

    #[test]
    fn headless_small_warehouse_completes() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 8,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 100,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert_eq!(record.tick_count, 100);
        assert_eq!(record.num_agents, 8);
        assert_eq!(record.throughput_series.len(), 100);
        assert_eq!(record.tasks_completed_series.len(), 100);
        assert!(record.total_tasks > 0, "agents should complete some tasks in 100 ticks");
    }

    #[test]
    fn headless_deterministic_same_seed() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 10,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 123,
            tick_count: 50,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let r1 = run_headless(&config);
        let r2 = run_headless(&config);
        assert_eq!(r1.total_tasks, r2.total_tasks, "same seed must produce identical results");
        assert_eq!(r1.throughput_series, r2.throughput_series);
        assert_eq!(r1.tasks_completed_series, r2.tasks_completed_series);
    }

    #[test]
    fn headless_zero_agents() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 0,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 10,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert_eq!(record.num_agents, 0);
        assert_eq!(record.total_tasks, 0);
    }

    #[test]
    fn headless_rhcr_solver() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 6,
            solver_name: "rhcr_pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 60,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert_eq!(record.num_agents, 6);
        assert!(record.total_tasks > 0);
    }

    #[test]
    fn headless_token_passing_solver() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 5,
            solver_name: "token_passing".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 60,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert_eq!(record.num_agents, 5);
        // Token passing may produce fewer tasks due to sequential planning overhead
    }

    #[test]
    fn headless_config_hash_deterministic() {
        let c1 = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 10,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 100,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let c2 = c1.clone();
        assert_eq!(c1.config_hash(), c2.config_hash());
    }

    #[test]
    fn headless_config_hash_differs_on_change() {
        let c1 = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 10,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 100,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let mut c2 = c1.clone();
        c2.seed = 43;
        assert_ne!(c1.config_hash(), c2.config_hash());
    }

    #[test]
    fn baseline_record_accessor_methods() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 5,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 30,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);

        // Accessors accept 1-based ticks. Should not panic and return reasonable values.
        let _ = record.throughput_at(0); // tick 0 = before sim, returns series[0] via saturating_sub
        let _ = record.throughput_at(1); // first tick
        let _ = record.throughput_at(15);
        let _ = record.throughput_at(1000); // beyond bounds — clamped
        let _ = record.tasks_at(1);
        let _ = record.idle_at(1);
    }

    #[test]
    fn headless_traffic_counts_populated() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 8,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 50,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert!(!record.traffic_counts.is_empty(), "traffic counts should be populated");
    }

    /// Parity test: two independent runs via SimulationRunner with identical config
    /// must produce exactly the same agent positions at every tick.
    #[test]
    fn headless_parity_tick_level_positions() {
        use crate::core::runner::SimulationRunner;

        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 12,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 999,
            tick_count: 80,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };

        fn run_and_record(config: &BaselineConfig) -> Vec<Vec<IVec2>> {
            let topo = ActiveTopology::from_name(&config.topology_name);
            let output = topo.topology().generate(config.seed);
            let grid = output.grid;
            let zones = output.zones;

            let solver = crate::solver::lifelong_solver_from_name(
                &config.solver_name,
                (grid.width * grid.height) as usize,
                config.num_agents,
            )
            .unwrap();

            let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
            let mut rng = SeededRng::new(config.seed);
            let agents = place_agents(config.num_agents, &grid, &zones, &mut rng);
            let fault_config =
                crate::fault::config::FaultConfig { enabled: false, ..Default::default() };
            let mut runner = SimulationRunner::new(
                grid,
                zones,
                agents,
                solver,
                rng,
                fault_config,
                crate::fault::scenario::FaultSchedule::default(),
            );
            let mut all_positions = Vec::with_capacity(config.tick_count as usize);

            for _tick in 0..config.tick_count {
                runner.tick(scheduler.scheduler(), &crate::core::queue::ClosestQueuePolicy);
                all_positions.push(runner.agents.iter().map(|a| a.pos).collect());
            }
            all_positions
        }

        let run1 = run_and_record(&config);
        let run2 = run_and_record(&config);

        for tick in 0..config.tick_count as usize {
            assert_eq!(run1[tick], run2[tick], "position divergence at tick {tick}");
        }
    }

    /// Parity test with closest scheduler — different code path in recycle_goals_core.
    #[test]
    fn headless_parity_closest_scheduler() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 20,
            solver_name: "pibt".into(),
            scheduler_name: "closest".into(),
            seed: 7777,
            tick_count: 60,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let r1 = run_headless(&config);
        let r2 = run_headless(&config);
        assert_eq!(r1.throughput_series, r2.throughput_series);
        assert_eq!(r1.tasks_completed_series, r2.tasks_completed_series);
        assert_eq!(r1.idle_count_series, r2.idle_count_series);
    }

    #[test]
    fn headless_no_vertex_collisions() {
        use crate::core::runner::SimulationRunner;

        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 15,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 100,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };

        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        let grid = output.grid;
        let zones = output.zones;

        let solver = crate::solver::lifelong_solver_from_name(
            &config.solver_name,
            (grid.width * grid.height) as usize,
            config.num_agents,
        )
        .unwrap();

        let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
        let mut rng = SeededRng::new(config.seed);
        let agents = place_agents(config.num_agents, &grid, &zones, &mut rng);
        let fault_config =
            crate::fault::config::FaultConfig { enabled: false, ..Default::default() };
        let mut runner = SimulationRunner::new(
            grid,
            zones,
            agents,
            solver,
            rng,
            fault_config,
            crate::fault::scenario::FaultSchedule::default(),
        );

        for tick in 0..config.tick_count {
            runner.tick(scheduler.scheduler(), &crate::core::queue::ClosestQueuePolicy);

            // Check no vertex collisions
            let mut seen = HashSet::new();
            for (i, a) in runner.agents.iter().enumerate() {
                assert!(
                    seen.insert(a.pos),
                    "vertex collision at tick {tick}: agent {i} at {:?}",
                    a.pos,
                );
            }
        }
    }

    /// Verify that baseline avg_throughput is non-zero and in a reasonable range.
    #[test]
    fn headless_avg_throughput_reasonable() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 8,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 200,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);
        assert!(record.avg_throughput > 0.0, "baseline avg throughput should be > 0");
        assert!(record.avg_throughput < 10.0, "baseline avg throughput should be reasonable");
    }

    /// Two independent SimulationRunners — one via `run_headless`, one manual —
    /// must produce identical metrics at every tick. This is the definitive parity
    /// test: both paths now use the same `SimulationRunner::tick()`.
    #[test]
    fn bridge_parity_live_vs_baseline_no_faults() {
        use crate::core::runner::SimulationRunner;

        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 15,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 200,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };

        // ── Run headless baseline (uses SimulationRunner internally) ─────
        let record = run_headless(&config);

        // ── Simulate the "live" path with a second SimulationRunner ──────
        let topo = ActiveTopology::from_name(&config.topology_name);
        let output = topo.topology().generate(config.seed);
        let grid = output.grid;
        let zones = output.zones;

        let solver = crate::solver::lifelong_solver_from_name(
            &config.solver_name,
            (grid.width * grid.height) as usize,
            config.num_agents,
        )
        .unwrap();

        let scheduler = ActiveScheduler::from_name(&config.scheduler_name);
        let mut rng = SeededRng::new(config.seed);
        let agents = place_agents(config.num_agents, &grid, &zones, &mut rng);
        let fault_config =
            crate::fault::config::FaultConfig { enabled: false, ..Default::default() };
        let mut runner = SimulationRunner::new(
            grid,
            zones,
            agents,
            solver,
            rng,
            fault_config,
            crate::fault::scenario::FaultSchedule::default(),
        );

        for _loop_tick in 0..config.tick_count {
            let result =
                runner.tick(scheduler.scheduler(), &crate::core::queue::ClosestQueuePolicy);

            // Accessors accept 1-based tick; result.tick is already 1-based.
            let tick = result.tick;

            // Throughput
            let live_tp = result.throughput;
            let bl_tp = record.throughput_at(tick);
            assert_eq!(
                live_tp, bl_tp,
                "throughput mismatch at tick={tick}: live={live_tp}, baseline={bl_tp}",
            );

            // Cumulative tasks
            let live_tasks = result.tasks_completed;
            let bl_tasks = record.tasks_at(tick);
            assert_eq!(
                live_tasks, bl_tasks,
                "tasks mismatch at tick={tick}: live={live_tasks}, baseline={bl_tasks}",
            );

            // Idle count
            let live_idle = result.idle_count;
            let bl_idle = record.idle_at(tick);
            assert_eq!(
                live_idle, bl_idle,
                "idle mismatch at tick={tick}: live={live_idle}, baseline={bl_idle}",
            );
        }
    }

    /// Verify throughput series contains only integer counts (0, 1, 2, …)
    /// and that the sum equals total_tasks.
    #[test]
    fn headless_throughput_is_instantaneous_count() {
        let config = BaselineConfig {
            topology_name: "warehouse_large".into(),
            num_agents: 8,
            solver_name: "pibt".into(),
            scheduler_name: "random".into(),
            seed: 42,
            tick_count: 100,
            grid_override: None,
            fault_enabled: false,
            agent_positions: None,
        };
        let record = run_headless(&config);

        // Every entry must be a non-negative integer
        for (i, &tp) in record.throughput_series.iter().enumerate() {
            assert!(tp >= 0.0, "tick {i}: throughput must be >= 0, got {tp}");
            assert_eq!(tp, tp.round(), "tick {i}: throughput must be integer, got {tp}");
        }

        // Sum of instantaneous counts must equal total tasks completed
        let sum: f64 = record.throughput_series.iter().sum();
        assert_eq!(
            sum as u64, record.total_tasks,
            "sum of per-tick throughput ({sum}) must equal total_tasks ({})",
            record.total_tasks
        );
    }
}
