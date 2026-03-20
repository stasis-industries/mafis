//! Headless simulation test harness.
//!
//! `SimHarness` boots a minimal Bevy `App` (no render, no UI) with the full
//! simulation stack: CorePlugin → SolverPlugin → FaultPlugin → AnalysisPlugin.
//! Render-dependent systems are excluded at compile time via `#[cfg(not(test))]`
//! in `AnalysisPlugin` and `FaultPlugin`.
//!
//! Usage:
//! ```rust
//! let mut h = SimHarness::new(4);   // 4 agents, 10×10 open grid
//! h.run_ticks(20);
//! assert!(h.tick() == 20);
//! ```
//!
//! Add new helpers here as the test suite grows — all test modules share this harness.

#![allow(dead_code)]

use bevy::prelude::*;

use crate::{
    analysis::{
        cascade::CascadeState,
        fault_metrics::FaultMetrics,
        heatmap::HeatmapState,
        history::TickHistory,
        scorecard::ResilienceScorecard,
        AnalysisPlugin,
    },
    core::{
        action::Action,
        agent::{AgentActionStats, AgentRegistry, LastAction, LogicalAgent},
        grid::GridMap,
        live_sim::LiveSim,
        phase::SimulationPhase,
        runner::SimAgent,
        state::{SimState, SimulationConfig},
        task::LifelongConfig,
        topology::ZoneMap,
        CorePlugin,
    },
    fault::{breakdown::Dead, config::FaultConfig, heat::HeatState, FaultPlugin},
    solver::SolverPlugin,
};

// ---------------------------------------------------------------------------
// SimHarness
// ---------------------------------------------------------------------------

/// Headless end-to-end simulation test harness.
///
/// Boots `MinimalPlugins` + the full simulation stack without any render or UI
/// plugins. Use [`run_ticks`] to advance the simulation tick-by-tick through
/// `FixedUpdate`. All analysis resources are accessible via typed accessors.
///
/// # Adding new tests
/// Create a new module under `src/sim_tests/` and declare it in `mod.rs`.
/// Build a `SimHarness`, run some ticks, then assert on the accessor outputs.
pub struct SimHarness {
    pub app: App,
}

impl SimHarness {
    // ── Construction ────────────────────────────────────────────────────────

    /// Build a harness with `n_agents` on a 10×10 open grid.
    ///
    /// Zone layout: pickup = row 0, delivery = row 9.
    /// Returns the harness already in `SimState::Running`.
    pub fn new(n_agents: usize) -> Self {
        let mut app = App::new();

        app.add_plugins((
            MinimalPlugins,
            // StatesPlugin provides the StateTransition schedule, which is
            // required by CorePlugin's init_state::<SimState>() but is NOT
            // included in MinimalPlugins (it is in DefaultPlugins).
            bevy::state::app::StatesPlugin,
            CorePlugin,
            SolverPlugin,
            FaultPlugin,
            AnalysisPlugin,
        ));

        // Run Startup systems.  `setup_heatmap_palette` is compiled out in
        // test builds (#[cfg(not(test))]) so no render assets are needed.
        app.update();

        // Override grid: compact 10×10 open floor.
        let (w, h) = (10i32, 10i32);
        app.world_mut().insert_resource(GridMap::new(w, h));

        // Minimal zone map: pickup row 0, delivery row h-1.
        let mut zones = ZoneMap::default();
        for x in 0..w {
            zones.pickup_cells.push(IVec2::new(x, 0));
            zones.delivery_cells.push(IVec2::new(x, h - 1));
        }
        app.world_mut().insert_resource(zones);

        // Spawn agents at distinct positions with opposite-corner goals.
        let n = n_agents.min((w * h) as usize);
        let mut entities = Vec::with_capacity(n);
        for i in 0..n {
            let x = (i % w as usize) as i32;
            let y = (i / w as usize) as i32;
            let start = IVec2::new(x, y);
            let goal = IVec2::new(w - 1 - x, h - 1 - y);

            let entity = app
                .world_mut()
                .spawn((
                    LogicalAgent::new(start, goal),
                    LastAction(Action::Wait),
                    AgentActionStats::default(),
                    HeatState::default(),
                ))
                .id();
            entities.push(entity);
        }

        // Register in AgentRegistry and attach AgentIndex component.
        for entity in entities {
            let index = app
                .world_mut()
                .resource_mut::<AgentRegistry>()
                .register(entity);
            app.world_mut().entity_mut(entity).insert(index);
        }

        // Disable faults by default for deterministic tests.
        // Use with_faults() to re-enable with zero breakdown probability.
        app.world_mut().resource_mut::<FaultConfig>().enabled = false;

        // Build SimulationRunner + LiveSim so drive_simulation actually runs.
        let runner_grid = app.world().resource::<GridMap>().clone();
        let runner_zones = app.world().resource::<ZoneMap>().clone();
        let runner_agents: Vec<SimAgent> = (0..n)
            .map(|i| {
                let x = (i % w as usize) as i32;
                let y = (i / w as usize) as i32;
                let mut sa = SimAgent::new(IVec2::new(x, y));
                sa.goal = IVec2::new(w - 1 - x, h - 1 - y);
                sa
            })
            .collect();
        let runner_solver = Box::new(crate::solver::pibt::PibtLifelongSolver::new());
        let runner_rng = crate::core::seed::SeededRng::new(42);
        let fault_config = app.world().resource::<FaultConfig>().clone();
        let runner = crate::core::runner::SimulationRunner::new(
            runner_grid, runner_zones, runner_agents, runner_solver,
            runner_rng, fault_config, Default::default(),
        );
        app.world_mut().insert_resource(LiveSim::new(runner, 1000));

        // Transition to Running and apply the state change + OnEnter handlers.
        app.world_mut()
            .resource_mut::<NextState<SimState>>()
            .set(SimState::Running);
        app.update();

        SimHarness { app }
    }

    /// Enable automatic fault generation (Weibull wear model).
    ///
    /// Uses very high eta so no faults trigger by default in tests.
    /// Set specific parameters directly via `h.app.world_mut()` when needed.
    pub fn with_faults(mut self) -> Self {
        let mut config = self.app.world_mut().resource_mut::<FaultConfig>();
        config.enabled = true;
        config.weibull_enabled = true;
        config.weibull_eta = 100_000.0; // very high so no faults in short tests
        self
    }

    // ── Simulation control ───────────────────────────────────────────────────

    /// Advance the simulation by exactly `n` `FixedUpdate` ticks, bypassing the
    /// time accumulator.  Each call runs all FixedUpdate systems once.
    pub fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.app.world_mut().run_schedule(FixedUpdate);
        }
    }

    /// Force the simulation phase (bypasses warmup timer).
    pub fn set_phase(&mut self, phase: SimulationPhase) {
        self.app.world_mut().insert_resource(phase);
    }

    // ── Resource accessors ───────────────────────────────────────────────────

    pub fn tick(&self) -> u64 {
        self.app.world().resource::<SimulationConfig>().tick
    }

    pub fn phase(&self) -> SimulationPhase {
        *self.app.world().resource::<SimulationPhase>()
    }

    pub fn fault_config(&self) -> &FaultConfig {
        self.app.world().resource::<FaultConfig>()
    }

    pub fn metrics(&self) -> &FaultMetrics {
        self.app.world().resource::<FaultMetrics>()
    }

    pub fn cascade(&self) -> &CascadeState {
        self.app.world().resource::<CascadeState>()
    }

    pub fn scorecard(&self) -> &ResilienceScorecard {
        self.app.world().resource::<ResilienceScorecard>()
    }

    pub fn history(&self) -> &TickHistory {
        self.app.world().resource::<TickHistory>()
    }

    pub fn heatmap(&self) -> &HeatmapState {
        self.app.world().resource::<HeatmapState>()
    }

    pub fn lifelong(&self) -> &LifelongConfig {
        self.app.world().resource::<LifelongConfig>()
    }

    pub fn sim_config(&self) -> &SimulationConfig {
        self.app.world().resource::<SimulationConfig>()
    }

    // ── Entity-level helpers ─────────────────────────────────────────────────

    /// Number of agents in the AgentRegistry (alive + dead).
    pub fn agent_count(&self) -> usize {
        self.app.world().resource::<AgentRegistry>().count()
    }

    /// Number of agents currently alive (no `Dead` component).
    pub fn alive_agent_count(&mut self) -> usize {
        self.app
            .world_mut()
            .query_filtered::<(), (With<LogicalAgent>, Without<Dead>)>()
            .iter(self.app.world())
            .count()
    }
}
