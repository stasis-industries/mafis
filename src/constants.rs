//! Global compile-time constants for MAFIS.
//!
//! **Change values here to tune the simulator.**
//! Every limit is documented with the maximum safe value for typical hardware.

// ── Simulation limits ────────────────────────────────────────────────

/// Maximum number of agents the UI slider allows.
#[cfg(target_arch = "wasm32")]
pub const MAX_AGENTS: usize = 1_000;
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_AGENTS: usize = 5_000;

/// Minimum number of agents.
pub const MIN_AGENTS: usize = 1;

/// Default agent count on startup.
pub const DEFAULT_AGENTS: usize = 5;

/// Maximum grid dimension (width or height) the UI slider allows.
/// Raised to 512 for MovingAI benchmark maps.
pub const MAX_GRID_DIM: i32 = 512;

/// Minimum grid dimension.
pub const MIN_GRID_DIM: i32 = 8;

/// Default grid width and height on startup.
pub const DEFAULT_GRID_DIM: i32 = 16;

// ── Loading (batched entity spawning) ───────────────────────────────

/// Obstacle entities spawned per frame during loading.
#[cfg(target_arch = "wasm32")]
pub const LOADING_OBSTACLE_BATCH: usize = 500;
#[cfg(not(target_arch = "wasm32"))]
pub const LOADING_OBSTACLE_BATCH: usize = 5_000;

/// Agent entities spawned per frame during loading.
#[cfg(target_arch = "wasm32")]
pub const LOADING_AGENT_BATCH: usize = 100;
#[cfg(not(target_arch = "wasm32"))]
pub const LOADING_AGENT_BATCH: usize = 1_000;

/// Baseline ticks computed per frame during loading.
/// Keeps the UI responsive while computing the headless baseline.
#[cfg(target_arch = "wasm32")]
pub const BASELINE_TICKS_PER_FRAME: u64 = 50;
#[cfg(not(target_arch = "wasm32"))]
pub const BASELINE_TICKS_PER_FRAME: u64 = 500;

// ── Rendering ────────────────────────────────────────────────────────

/// Number of steps in each heatmap tile color gradient (density & traffic).
pub const HEATMAP_PALETTE_STEPS: usize = 8;

/// Grid dimensions above which grid line entities are NOT spawned.
/// At 128×128, 258 line entities are invisible noise.
pub const GRID_LINE_THRESHOLD: i32 = 64;

// ── Analysis ─────────────────────────────────────────────────────────

/// Maximum BFS cascade depth. Deeper cascades are truncated.
pub const MAX_CASCADE_DEPTH: u32 = 10;

/// Maximum entries kept in the fault survival time-series.
pub const MAX_SURVIVAL_ENTRIES: usize = 1000;

/// ADG (Action Dependency Graph) lookahead steps per agent.
pub const ADG_LOOKAHEAD: usize = 3;

/// ADG throttle tiers: tick stride per agent-count bracket.
/// Below TIER_SMALL → every tick. Between tiers → stride N. Above last tier → XLARGE stride.
pub const ADG_STRIDE_SMALL: u64 = 1;   // ≤100 agents: every tick
pub const ADG_STRIDE_MED: u64 = 3;     // 101–300 agents: every 3 ticks
pub const ADG_STRIDE_LARGE: u64 = 5;   // 301–500 agents: every 5 ticks
pub const ADG_STRIDE_XLARGE: u64 = 8;  // 500+ agents: every 8 ticks

/// Agent count thresholds for ADG stride tiers.
pub const ADG_TIER_SMALL: usize = 100;
pub const ADG_TIER_MED: usize = 300;
pub const ADG_TIER_LARGE: usize = 500;

/// How often (ticks) to run full betweenness centrality. 0 = disabled.
pub const BETWEENNESS_INTERVAL: u64 = 50;

/// Agent count above which betweenness is disabled (too expensive).
pub const BETWEENNESS_AGENT_LIMIT: usize = 200;

/// Sliding window size for throughput calculation (goals per tick).
pub const THROUGHPUT_WINDOW_SIZE: usize = 100;

// ── Simulation duration ──────────────────────────────────────────────
pub const DURATION_SHORT: u64 = 200;
pub const DURATION_MEDIUM: u64 = 500;
pub const DURATION_LONG: u64 = 1000;
pub const DEFAULT_DURATION: u64 = 500;
pub const MIN_DURATION: u64 = 50;
pub const MAX_DURATION: u64 = 5000;

// ── Bridge / serialization ───────────────────────────────────────────

/// Agent count above which the bridge sends aggregate summaries.
pub const AGGREGATE_THRESHOLD: usize = 50;

/// Bridge sync interval for small agent counts (≤ AGGREGATE_THRESHOLD).
pub const BRIDGE_SYNC_INTERVAL_FAST: f32 = 0.09;

/// Bridge sync interval for medium agent counts (AGGREGATE_THRESHOLD+1 – 200).
pub const BRIDGE_SYNC_INTERVAL_MED: f32 = 0.15;

/// Bridge sync interval for large agent counts (201–400).
pub const BRIDGE_SYNC_INTERVAL_SLOW: f32 = 0.50;

/// Bridge sync interval for very large agent counts (400+).
pub const BRIDGE_SYNC_INTERVAL_XLARGE: f32 = 1.0;

// ── Manual fault injection ──────────────────────────────────────────

/// How often (ticks) to re-invoke the solver when agents are stuck (0 = disabled).
pub const REPLAN_INTERVAL: u64 = 20;

/// PIBT planning horizon for lifelong replans. Short horizon because goals
/// change constantly — computing 1000 steps wastes ~98% of work.
pub const LIFELONG_PLAN_HORIZON: u64 = 20;

// ── RHCR (Rolling-Horizon Collision Resolution) ───────────────────

/// Maximum planning horizon (H). Beyond this, windowed planning degrades.
pub const RHCR_MAX_HORIZON: usize = 40;

/// Minimum planning horizon. Below this, plans are too short to be useful.
pub const RHCR_MIN_HORIZON: usize = 5;

/// Maximum replan interval (W). W > H makes no sense.
pub const RHCR_MAX_REPLAN_INTERVAL: usize = 40;

/// Minimum replan interval. Every-tick defeats RHCR's purpose.
pub const RHCR_MIN_REPLAN_INTERVAL: usize = 2;

/// Maximum PBS tree nodes before aborting (memory + compute safeguard).
#[cfg(target_arch = "wasm32")]
pub const PBS_MAX_NODE_LIMIT: usize = 1_000;
#[cfg(not(target_arch = "wasm32"))]
pub const PBS_MAX_NODE_LIMIT: usize = 10_000;

/// Maximum spacetime horizon for Token Passing A*. Increase if large topologies
/// produce NoSolution on paths that require more than 200 steps.
pub const TOKEN_PATH_MAX_TIME: u64 = 300;

/// Maximum spacetime A* time horizon per agent per tick in Token Passing.
/// Lower = faster but may fail to find paths in dense/large grids.
pub const TOKEN_ASTAR_MAX_TIME: u64 = 200;

/// Maximum number of nodes expanded per spacetime A* call.
/// Without this, the A* explores up to cells × timesteps states (e.g., 160K
/// on a 40×20 grid with time horizon 200). With 40 agents all failing, that's
/// 6.4M expansions per tick — catastrophic in WASM.
/// 5000 is enough for paths up to ~80 steps on uncongested grids.
pub const ASTAR_MAX_EXPANSIONS: u64 = 5_000;

/// Default duration for latency injection (ticks).
pub const DEFAULT_LATENCY_DURATION: u32 = 20;

// ── Tick history ────────────────────────────────────────────────────

/// Maximum number of tick snapshots stored for rewind.
/// At 500 agents × 48 bytes/snapshot, 1000 entries ≈ 24 MB (vs 120 MB at 5000).
pub const MAX_TICK_HISTORY: usize = 1000;

/// Record a snapshot every N ticks (1 = every tick, 5 = every 5th tick).
/// Reduces per-tick allocation by (N-1)/N. Value of 3 = ~67% fewer allocations.
pub const TICK_SNAPSHOT_INTERVAL: u64 = 3;

// ── Resilience scorecard ────────────────────────────────────────────

/// Critical Time threshold: fraction of baseline avg throughput below which
/// the system is considered "in critical state."
/// Inspired by performability theory (Ghasemieh & Haverkort); threshold configurable.
/// Default 0.5 = "half of normal throughput."
pub const CRITICAL_TIME_THRESHOLD: f64 = 0.5;

/// How often to recompute scorecard metrics (ticks).
pub const SCORECARD_RECOMPUTE_INTERVAL: u64 = 50;

/// Moving average window for throughput chart smoothing (ticks).
pub const THROUGHPUT_MA_WINDOW: usize = 10;
