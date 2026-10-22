//! RHCR-PBS limits and reference defaults.

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

/// PBS high-level node budget per agent. The reference stops PBS on a 60 s
/// clock (driver.cpp:108); MAFIS needs a deterministic budget. Once A* stopped
/// returning unholdable end cells, solved windows needed at most 23 nodes at
/// DD n=40 (10 seeds) and 54 at DD n=80, well inside 3n, while a 10,000-node
/// budget made one DD n=120 window run for hours. The budget therefore only
/// cuts off windows PBS would not solve; those go to LRA and are counted in
/// the `pbs_partial_rate` telemetry.
pub const PBS_NODES_PER_AGENT: usize = 3;

/// Floor on the PBS node budget for small fleets.
pub const PBS_MIN_NODE_LIMIT: usize = 50;

/// Maximum PBS tree nodes before aborting (memory + compute safeguard).
pub const PBS_MAX_NODE_LIMIT: usize = 10_000;
const _: () = assert!(PBS_MAX_NODE_LIMIT >= 5000, "PBS_MAX_NODE_LIMIT should be >= 5000");
const _: () = assert!(PBS_MAX_NODE_LIMIT <= 1_000_000, "PBS_MAX_NODE_LIMIT is unreasonably high");

// ── RHCR / PBS (reference-aligned) ──────────────────────────────────
//
// Constants for the faithful PBS port.
// Each value cites a specific line in the canonical Jiaoyang-Li/RHCR
// reference (`docs/papers_codes/rhcr/`) so reviewers can verify fidelity.

/// Default RHCR replan interval (W).
/// REFERENCE: `docs/papers_codes/rhcr/src/driver.cpp:116` — `simulation_window`.
pub const RHCR_SIMULATION_WINDOW_DEFAULT: usize = 5;

/// Default RHCR planning window (H): conflicts resolved for the next 10 ticks.
/// REFERENCE: RHCR README example `--simulation_window=5 --planning_window=10`
/// (driver.cpp:118 defaults to unbounded).
pub const RHCR_PLANNING_HORIZON_DEFAULT: usize = 10;

/// Per-call expansion cap for PBS's sequential A* (deterministic; the
/// reference only has the global solver time limit). Non-binding at the
/// default planning window: a single-goal search over w = 10 can close at
/// most sum_{t=0..10}(2t^2 + 2t + 1) = 891 (cell, time) states. Binds only
/// for w >= 14.
pub const PBS_ASTAR_MAX_EXPANSIONS: u64 = 2_000;

/// PBS `find_consistent_paths` cycle guard: abort cascade replan if
/// `count > paths.len() * MULT`.
/// REFERENCE: `docs/papers_codes/rhcr/src/PBS.cpp:423`.
pub const PBS_CONSISTENT_PATHS_REPLAN_MULT: usize = 5;

/// Whether PBS uses lazy priority resolution.
/// REFERENCE: `docs/papers_codes/rhcr/src/driver.cpp:114` — reference default
/// `lazyP = false` (i.e. eager mode is the canonical default).
pub const PBS_LAZY_PRIORITY_DEFAULT: bool = false;

/// Whether PBS's `find_replan_agents` skips agents still waiting at start.
/// REFERENCE: `docs/papers_codes/rhcr/inc/PBS.h:13` — `prioritize_start = true`
/// declaration.
pub const PBS_PRIORITIZE_START_DEFAULT: bool = true;
