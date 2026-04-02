use bevy::prelude::IVec2;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashMap, HashSet};

use super::super::topology::ZoneMap;
use super::{TaskLeg, TaskScheduler};

// ---------------------------------------------------------------------------
// Types used by recycle_goals_core
// ---------------------------------------------------------------------------

/// Agent snapshot for the recycle_goals core function.
pub struct TaskAgentSnapshot {
    pub pos: IVec2,
    pub goal: IVec2,
    pub task_leg: TaskLeg,
    pub alive: bool,
    /// Whether the agent is frozen (latency injection). Frozen agents' goals
    /// are excluded from `used_goals` so they don't block cell assignment for
    /// active agents, and they skip state transitions until latency expires.
    pub frozen: bool,
}

/// Per-agent update from recycle_goals_core.
pub struct TaskUpdate {
    pub task_leg: TaskLeg,
    pub goal: IVec2,
    pub path_cleared: bool,
}

/// Aggregate result from recycle_goals_core.
pub struct RecycleResult {
    pub updates: Vec<TaskUpdate>,
    pub completion_ticks: Vec<u64>,
    pub needs_replan: bool,
    /// Agent indices that just entered Loading this tick (must NOT be
    /// processed by the queue manager on the same tick — ensures Loading
    /// is visible for at least 1 tick).
    pub just_loaded: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Core recycle_goals (shared by ECS and headless baseline)
// ---------------------------------------------------------------------------

/// Pure task recycling logic: checks agents at goal, assigns new tasks.
///
/// Both the live ECS system and headless baseline call this.
/// Agents MUST be pre-sorted by index before calling.
///
/// State transitions enforce a 1-tick minimum dwell:
/// - `TravelLoaded` → `Free` (not immediately `TravelEmpty`)
/// - `TravelEmpty` → `Loading` (queue manager must wait 1 tick via `just_loaded`)
///
/// This ensures the user can observe each state in the UI.
///
/// # Task assignment model
///
/// Task creation (which pickup cells exist) is always random. The scheduler
/// controls only task *assignment* (which agent goes to which task). All Free
/// agents are batch-assigned via `TaskScheduler::assign_pickups_batch` so that
/// schedulers like `ClosestFirst` can implement proper random-create/closest-assign
/// semantics instead of converging on a positional hotspot.
pub fn recycle_goals_core(
    agents: &[TaskAgentSnapshot],
    scheduler: &dyn TaskScheduler,
    zones: &ZoneMap,
    rng: &mut ChaCha8Rng,
    tick: u64,
) -> RecycleResult {
    // Frozen (latency-injected) agents are excluded: their goals are stale
    // and they can't reach them, so reserving those cells only shrinks the
    // available pool for active agents — causing artificial cell starvation
    // (e.g., 4 active agents cycling on 2 cells during a zone outage).
    let mut used_goals: HashSet<IVec2> =
        agents.iter().filter(|a| a.alive && !a.frozen && a.pos != a.goal).map(|a| a.goal).collect();

    let mut updates: Vec<TaskUpdate> = agents
        .iter()
        .map(|a| TaskUpdate { task_leg: a.task_leg.clone(), goal: a.goal, path_cleared: false })
        .collect();

    let mut completion_ticks = Vec::new();
    let mut needs_replan = false;
    let mut just_loaded = Vec::new();

    // ── Pass 1: batch-assign pickups to all Free agents ──────────────────────
    // Collect every alive, non-frozen Free agent that has reached its goal.
    let mut free_agents: Vec<(usize, IVec2)> = agents
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            a.alive && !a.frozen && a.pos == a.goal && matches!(a.task_leg, TaskLeg::Free)
        })
        .map(|(i, a)| (i, a.pos))
        .collect();

    // Shuffle so lower-index agents don't always get first pick.
    // Fisher-Yates in-place — O(n), negligible cost.
    for i in (1..free_agents.len()).rev() {
        let j = rng.random_range(0..=i);
        free_agents.swap(i, j);
    }

    // The scheduler generates random task candidates and assigns them.
    // used_goals is updated in-place so subsequent passes see occupied cells.
    let pickup_assignments: HashMap<usize, IVec2> = scheduler
        .assign_pickups_batch(&free_agents, zones, &mut used_goals, rng)
        .into_iter()
        .collect();

    if !pickup_assignments.is_empty() {
        needs_replan = true;
    }

    // ── Pass 2: apply assignments and process all other state transitions ─────
    for (i, agent) in agents.iter().enumerate() {
        // Skip dead or frozen agents — they must not consume scheduler
        // assignments or transition state while under latency injection.
        if !agent.alive || agent.frozen {
            continue;
        }

        if agent.pos != agent.goal {
            continue;
        }

        match &agent.task_leg {
            TaskLeg::Free => {
                if let Some(&pickup) = pickup_assignments.get(&i) {
                    updates[i].task_leg = TaskLeg::TravelEmpty(pickup);
                    updates[i].goal = pickup;
                    updates[i].path_cleared = true;
                }
                // If no assignment (all pickups claimed), agent stays Free.
            }
            TaskLeg::TravelEmpty(pickup_cell) => {
                // Transition to Loading — queue manager handles delivery assignment
                // on the NEXT tick (via just_loaded skip set).
                let pickup = *pickup_cell;
                updates[i].task_leg = TaskLeg::Loading(pickup);
                just_loaded.push(i);
            }
            TaskLeg::Loading(_) => {
                // Loading agents wait for queue manager to assign a delivery queue.
                // No action here — QueueManager::tick() processes Loading → TravelToQueue.
            }
            TaskLeg::TravelToQueue { .. } => {
                // TravelToQueue agents are heading to the back of a queue line.
                // Managed by QueueManager (arrivals → Queuing).
            }
            TaskLeg::Queuing { .. } => {
                // Queuing agents are physically in a queue slot (compact, promote).
                // Managed by QueueManager.
            }
            TaskLeg::TravelLoaded { .. } => {
                // Delivery complete → transition to Free. Do NOT immediately
                // assign a new pickup — that would make Free a 0-tick state.
                // The next tick's Free→TravelEmpty handles reassignment.
                updates[i].task_leg = TaskLeg::Free;
                completion_ticks.push(tick);
                needs_replan = true;
            }
            TaskLeg::Unloading { .. } => {}
            TaskLeg::Charging => {}
        }
    }

    RecycleResult { updates, completion_ticks, needs_replan, just_loaded }
}
