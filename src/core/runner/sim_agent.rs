//! Per-agent state and result types for the simulation runner.

use std::collections::VecDeque;

use bevy::math::IVec2;
use smallvec::SmallVec;

use crate::core::action::Action;
use crate::core::task::TaskLeg;
use crate::fault::config::{FaultSource, FaultType};

// ---------------------------------------------------------------------------
// SimAgent — plain struct, owns all per-agent simulation state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SimAgent {
    pub pos: IVec2,
    pub goal: IVec2,
    pub planned_path: VecDeque<Action>,
    pub task_leg: TaskLeg,
    /// Accumulated heat (wear-based fault model).
    pub heat: f32,
    /// Whether the agent is alive (false = dead/broken down).
    pub alive: bool,
    /// Remaining ticks of latency injection (forces Wait).
    pub latency_remaining: u32,
    /// Last action taken (for heat accumulation).
    pub last_action: Action,
    /// Ticks alive, the clock of the Weibull failure model.
    /// Increments only on Move actions -- captures mechanical wear from distance traveled.
    /// Basis: mechanical wear (encoders, tires, gears) dominates field robot
    /// failures and is distance-proportional (Carlson & Murphy 2005).
    pub operational_age: u32,
    /// Tick at which this agent's next intermittent fault fires.
    /// None = not yet initialized; sampled lazily on first intermittent fault check.
    pub next_fault_tick: Option<u64>,
    /// Whether the agent was forced to wait by collision resolution last tick.
    pub last_was_forced: bool,
}

impl SimAgent {
    pub fn new(start: IVec2) -> Self {
        Self {
            pos: start,
            goal: start,
            planned_path: VecDeque::new(),
            task_leg: TaskLeg::Free,
            heat: 0.0,
            alive: true,
            latency_remaining: 0,
            last_action: Action::Wait,
            operational_age: 0,
            next_fault_tick: None,
            last_was_forced: false,
        }
    }

    pub fn has_plan(&self) -> bool {
        !self.planned_path.is_empty()
    }

    pub fn has_reached_goal(&self) -> bool {
        self.pos == self.goal
    }
}

// ---------------------------------------------------------------------------
// SimCommand — commands queued from JS/bridge, processed at tick boundary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum SimCommand {
    /// Kill an agent by index — marks dead, places obstacle.
    KillAgent { index: usize, source: FaultSource },
    /// Place a permanent obstacle at a grid cell.
    PlaceObstacle(IVec2),
    /// Inject latency on an agent — forces Wait for `duration` ticks.
    InjectLatency { agent_index: usize, duration: u32, source: FaultSource },
}

// ---------------------------------------------------------------------------
// FaultRecord — fault event produced during a tick
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct FaultRecord {
    pub agent_index: usize,
    pub fault_type: FaultType,
    pub source: FaultSource,
    pub tick: u64,
    pub position: IVec2,
    /// Number of other alive agents whose planned paths cross the dead cell.
    /// Computed at the instant of death, before `replan_after_fault` clears the
    /// evidence. This captures obstacle-creation cascade impact that the ADG-based
    /// BFS misses (because it runs post-replan).
    pub paths_invalidated: u32,
}

// ---------------------------------------------------------------------------
// FaultSnapshot — agent state at the instant a group of faults is applied
// ---------------------------------------------------------------------------

/// Agent state captured when a group of simultaneous faults is applied —
/// before any victim is removed or any plan is cleared — so cascade analysis
/// sees the moves the victims' neighbours were about to execute.
#[derive(Clone, Debug, Default)]
pub struct FaultSnapshot {
    /// Agents faulted at this instant (multi-source cascade BFS roots).
    pub sources: Vec<usize>,
    /// Per agent: cells occupied at plan steps `0..=lookahead` (step 0 = current
    /// position). Empty for agents already dead.
    pub cells: Vec<SmallVec<[IVec2; 4]>>,
}

impl FaultSnapshot {
    /// Capture every agent's next `lookahead` planned cells; `sources` is
    /// filled by the caller once the victims are known.
    pub fn capture(agents: &[SimAgent], lookahead: usize) -> Self {
        let cells = agents
            .iter()
            .map(|a| {
                if !a.alive {
                    return SmallVec::new();
                }
                let mut pos = a.pos;
                std::iter::once(pos)
                    .chain(a.planned_path.iter().take(lookahead).map(|action| {
                        pos = action.apply(pos);
                        pos
                    }))
                    .collect()
            })
            .collect();
        Self { sources: Vec::new(), cells }
    }
}

// ---------------------------------------------------------------------------
// TickResult — everything the caller needs after one tick
// ---------------------------------------------------------------------------

/// Per-agent result from a single tick.
pub struct AgentTickResult {
    pub new_pos: IVec2,
    pub action: Action,
    pub was_forced: bool,
}

/// Aggregate result from `SimulationRunner::tick()`.
pub struct TickResult {
    /// Per-agent move results (same order as runner.agents).
    pub moves: Vec<AgentTickResult>,
    /// Ticks at which tasks were completed this tick.
    pub completion_ticks: Vec<u64>,
    /// Tasks completed so far (cumulative).
    pub tasks_completed: u64,
    /// Instantaneous throughput at this tick.
    pub throughput: f64,
    /// Current tick number (after increment).
    pub tick: u64,
    /// Number of idle agents after task recycling.
    pub idle_count: usize,
    /// Whether all alive agents have reached their goals.
    pub all_at_goal: bool,
    /// Fault events that occurred this tick.
    pub fault_events: Vec<FaultRecord>,
    /// Number of alive agents after this tick.
    pub alive_count: usize,
    /// Number of dead agents after this tick.
    pub dead_count: usize,
    /// Average heat across alive agents.
    pub heat_avg: f32,
}
