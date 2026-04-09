//! Global compile-time constants for MAFIS.
//!
//! **Change values here to tune the simulator.**
//! Every limit is documented with the maximum safe value for typical hardware.

// ── Version ─────────────────────────────────────────────────────────

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Maximum queued JS→WASM commands before new ones are dropped.
pub const MAX_COMMAND_QUEUE: usize = 256;

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

/// Maximum entries kept in `FaultMetrics::event_records` and the related
/// `cascade_spreads` / `recovery_times` deques. Bounds memory and the cost
/// of any iteration over the full history (e.g., bridge serialization
/// already takes only the last 100; desktop UI takes only the last 10).
///
/// Statistical aggregates (avg cascade spread, MTTR, MTBF, propagation rate)
/// are tracked via running sums on `FaultMetrics`, so they remain accurate
/// across the full simulation even when older entries are evicted.
pub const MAX_FAULT_EVENT_HISTORY: usize = 1000;

/// ADG (Action Dependency Graph) lookahead steps per agent.
pub const ADG_LOOKAHEAD: usize = 3;

/// ADG throttle tiers: tick stride per agent-count bracket.
/// Below TIER_SMALL → every tick. Between tiers → stride N. Above last tier → XLARGE stride.
/// Empirically tuned to keep ADG computation under ~1ms/tick at each bracket.
/// At 500 agents on a 32x21 grid, stride 8 samples 12.5% of ticks (sufficient
/// for detecting cascading delays without dominating frame time).
pub const ADG_STRIDE_SMALL: u64 = 1; // ≤100 agents: every tick
pub const ADG_STRIDE_MED: u64 = 3; // 101–300 agents: every 3 ticks
pub const ADG_STRIDE_LARGE: u64 = 5; // 301–500 agents: every 5 ticks
pub const ADG_STRIDE_XLARGE: u64 = 8; // 500+ agents: every 8 ticks

/// Agent count thresholds for ADG stride tiers.
pub const ADG_TIER_SMALL: usize = 100;
pub const ADG_TIER_MED: usize = 300;
pub const ADG_TIER_LARGE: usize = 500;

/// How often (ticks) to run full betweenness centrality. 0 = disabled.
/// Brandes' algorithm is O(VE); at 100 agents on a 32x21 grid (~670 cells),
/// one pass takes ~2ms. Every 50 ticks = ~40ms amortized cost per second at 20Hz.
pub const BETWEENNESS_INTERVAL: u64 = 50;

/// Agent count above which betweenness is disabled (too expensive).
pub const BETWEENNESS_AGENT_LIMIT: usize = 200;

/// Max entries in the completion-tick deques (runner + LifelongConfig).
/// The deque stores tick numbers for snapshot/rewind — the cap is a memory
/// bound, not an averaging window.
pub const THROUGHPUT_WINDOW_SIZE: usize = 100;

// ── Simulation duration ──────────────────────────────────────────────
pub const DURATION_SHORT: u64 = 200;
pub const DURATION_MEDIUM: u64 = 500;
pub const DURATION_LONG: u64 = 1000;
pub const DEFAULT_DURATION: u64 = 500;
pub const MIN_DURATION: u64 = 50;
pub const MAX_DURATION: u64 = 1_000_000;

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

/// Maximum planning horizon (H). Li et al. 2021 use H=20 for dense warehouses;
/// 40 allows larger maps where agents need longer paths to reach goals.
pub const RHCR_MAX_HORIZON: usize = 40;

/// Minimum planning horizon. Below 5, plans are too short for agents to clear
/// even simple intersections (average aisle length ~4 cells in compact grids).
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

// ── RHCR / PBS (reference-aligned) ──────────────────────────────────
//
// Constants for the faithful PBS port (PAAMS 2026 RHCR-PBS fix).
// Each value cites a specific line in the canonical Jiaoyang-Li/RHCR
// reference (`docs/papers_codes/rhcr/`) so reviewers can verify fidelity.

/// Default RHCR replan interval (W).
/// REFERENCE: `docs/papers_codes/rhcr/src/driver.cpp:116` — `simulation_window`.
pub const RHCR_SIMULATION_WINDOW_DEFAULT: usize = 5;

/// Default RHCR planning horizon (H).
/// REFERENCE: Li et al. 2021 §5.1 — typical `w + h` lookahead used in the
/// published warehouse experiments.
pub const RHCR_PLANNING_HORIZON_DEFAULT: usize = 10;

/// PBS `find_consistent_paths` cycle guard: abort cascade replan if
/// `count > paths.len() * MULT`.
/// REFERENCE: `docs/papers_codes/rhcr/src/PBS.cpp:423`.
pub const PBS_CONSISTENT_PATHS_REPLAN_MULT: usize = 5;

/// Maximum length of the per-agent goal sequence used by `WindowAgent`
/// and the peek chain. Worst case: `RHCR_SIMULATION_WINDOW_DEFAULT` (=5)
/// minimum-cost legs of length 1 plus a 3-step safety margin. Matches
/// the unbounded `KivaSystem.cpp` `goal_locations` vector with a sane cap.
pub const PBS_GOAL_SEQUENCE_MAX_LEN: usize = 8;

/// Whether PBS uses lazy priority resolution.
/// REFERENCE: `docs/papers_codes/rhcr/src/driver.cpp:114` — reference default
/// `lazyP = false` (i.e. eager mode is the canonical default).
pub const PBS_LAZY_PRIORITY_DEFAULT: bool = false;

/// Whether PBS's `find_replan_agents` skips agents still waiting at start.
/// REFERENCE: `docs/papers_codes/rhcr/inc/PBS.h:13` — `prioritize_start = true`
/// declaration.
pub const PBS_PRIORITIZE_START_DEFAULT: bool = true;

/// Maximum consecutive rejections in `peek_task_chain` before terminating
/// the chain. Mirrors the implicit retry budget in `KivaSystem.cpp:143–196`
/// where `update_goal_locations` retries on `next == curr`.
pub const PEEK_CHAIN_MAX_RETRIES: usize = 10;

/// Maximum spacetime horizon for Token Passing A*. Increase if large topologies
/// produce NoSolution on paths that require more than 200 steps.
pub const TOKEN_PATH_MAX_TIME: u64 = 300;

/// Maximum spacetime A* time horizon per agent per tick in Token Passing.
/// Lower = faster but may fail to find paths in dense/large grids.
pub const TOKEN_ASTAR_MAX_TIME: u64 = 200;

/// Maximum number of nodes expanded per spacetime A* call.
/// Without this, A* explores up to cells × timesteps states (e.g., 160K
/// on a 40x20 grid with time horizon 200). With 40 agents all failing, that's
/// 6.4M expansions per tick — catastrophic in WASM.
/// 5000 is enough for paths up to ~80 steps on uncongested grids.
/// Empirically validated: at 5000, Token Passing finds valid paths for 95%+ of
/// agents on warehouse_large with 100 agents.
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
///
/// 50% is an operational rule of thumb consistent with service-level objective
/// practice in fleet operations (e.g., Amazon Robotics SLA conventions trigger
/// operator intervention when throughput drops below half of nominal). It is
/// NOT a derived constant from a specific performability paper
pub const CRITICAL_TIME_THRESHOLD: f64 = 0.5;

/// How often to recompute scorecard metrics (ticks).
pub const SCORECARD_RECOMPUTE_INTERVAL: u64 = 50;

/// Moving average window for throughput chart smoothing (ticks).
pub const THROUGHPUT_MA_WINDOW: usize = 10;
