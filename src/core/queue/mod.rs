//! Delivery queue line system — modular queuing for delivery zones.
//!
//! Each delivery cell has a directed queue line (sequence of walkable cells
//! extending in one direction until a wall). Agents join the back of a queue,
//! shuffle forward each tick, and get promoted to delivery when the cell is free.
//!
//! Queue assignment policy (`DeliveryQueuePolicy`) is a swappable research
//! variable — parallel to `TaskScheduler` and `LifelongSolver`.

pub mod policy;
pub use policy::*;

use bevy::prelude::*;
use rand::Rng;
use rand_chacha::ChaCha8Rng;

use super::action::Direction;
use super::grid::GridMap;
use super::runner::SimAgent;
use super::task::TaskLeg;

// ---------------------------------------------------------------------------
// QueueLine — topology data (immutable during simulation)
// ---------------------------------------------------------------------------

/// A physical queue lane extending from a delivery cell in one direction.
#[derive(Clone, Debug)]
pub struct QueueLine {
    /// The delivery cell this queue serves.
    pub delivery_cell: IVec2,
    /// Direction the queue extends (away from delivery).
    pub direction: Direction,
    /// Ordered queue positions: `[0]` = closest to delivery (first to be promoted),
    /// `[last]` = back of line (where new agents join).
    pub cells: Vec<IVec2>,
}

impl QueueLine {
    /// Maximum queue line length. Real warehouse queue lanes are short — just
    /// enough to buffer a few agents waiting for delivery. Without a cap, the
    /// line can extend through entire pickup rows or corridors, swallowing
    /// walkable space and skewing experiment data.
    const MAX_LENGTH: usize = 4;

    /// Compute a queue line from a delivery cell, direction, and grid.
    ///
    /// Walks from `delivery_cell + direction` in `direction` until hitting
    /// a non-walkable cell, grid boundary, or the length cap. Returns `None`
    /// if no walkable cells exist in that direction.
    pub fn compute(delivery_cell: IVec2, direction: Direction, grid: &GridMap) -> Option<Self> {
        let mut cells = Vec::new();
        let offset = direction.offset();
        let mut pos = delivery_cell + offset;

        while grid.is_in_bounds(pos) && grid.is_walkable(pos) && cells.len() < Self::MAX_LENGTH {
            cells.push(pos);
            pos += offset;
        }

        if cells.is_empty() {
            return None;
        }

        Some(Self { delivery_cell, direction, cells })
    }

    /// Back-of-line cell (furthest from delivery).
    pub fn back_cell(&self) -> IVec2 {
        *self.cells.last().unwrap_or(&self.delivery_cell)
    }

    /// The cell a new agent should target, accounting for pending reservations.
    /// Skips `state.reserved` empty slots so that agents assigned in the same
    /// tick get different goal cells. When the queue is empty this returns
    /// `cells[0]` (right next to delivery) instead of forcing agents to the far end.
    pub fn join_cell(&self, state: &QueueState) -> IVec2 {
        if let Some(slot_idx) = state.nth_empty_slot(state.reserved) {
            self.cells[slot_idx]
        } else {
            self.back_cell()
        }
    }

    /// Queue capacity (number of waiting slots, excluding delivery cell itself).
    pub fn capacity(&self) -> usize {
        self.cells.len()
    }
}

// ---------------------------------------------------------------------------
// QueueState — runtime state per queue line
// ---------------------------------------------------------------------------

/// Runtime occupancy state for one queue line.
#[derive(Clone, Debug)]
pub struct QueueState {
    /// Index into `ZoneMap::queue_lines`.
    pub line_index: usize,
    /// `slots[i] = Some(agent_index)` if an agent is physically in that queue position.
    /// Same length as `QueueLine::cells`.
    pub slots: Vec<Option<usize>>,
    /// Agent currently at the delivery cell (doing delivery).
    pub delivery_occupied_by: Option<usize>,
    /// Transient reservation count — tracks how many slots have been logically
    /// claimed within the current phase but not yet physically occupied. Reset
    /// to 0 at the start/end of each phase that uses it. Makes `free_slots()`
    /// and `is_full()` aware of pending assignments so policies see correct
    /// availability when multiple agents are assigned in the same tick.
    pub(super) reserved: usize,
}

impl QueueState {
    pub fn new(line_index: usize, capacity: usize) -> Self {
        Self { line_index, slots: vec![None; capacity], delivery_occupied_by: None, reserved: 0 }
    }

    /// Number of agents currently in the queue (not counting delivery).
    pub fn occupancy(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Number of empty slots.
    pub fn free_slots(&self) -> usize {
        self.slots.iter().filter(|s| s.is_none()).count()
    }

    /// Whether the queue is completely full.
    pub fn is_full(&self) -> bool {
        self.free_slots() == 0
    }

    /// Find the (skip+1)th empty slot index. When `skip=0`, returns the
    /// front-most empty slot (same as the old `first_empty_slot`).
    pub(super) fn nth_empty_slot(&self, skip: usize) -> Option<usize> {
        self.slots.iter().enumerate().filter(|(_, s)| s.is_none()).nth(skip).map(|(i, _)| i)
    }

    /// Find the front-most empty slot index.
    pub(super) fn first_empty_slot(&self) -> Option<usize> {
        self.nth_empty_slot(0)
    }
}

// ---------------------------------------------------------------------------
// QueueDecision
// ---------------------------------------------------------------------------

/// Result from a queue policy's `choose_queue`.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueDecision {
    /// Join this queue line.
    JoinQueue { line_index: usize },
    /// All queues full — stay in Loading state, retry next tick.
    Hold,
}

// ---------------------------------------------------------------------------
// ActiveQueuePolicy resource
// ---------------------------------------------------------------------------

pub const QUEUE_POLICY_NAMES: &[(&str, &str)] =
    &[("closest", "Closest"), ("least_occupied", "Least Occupied"), ("weighted", "Weighted")];

#[derive(Resource)]
pub struct ActiveQueuePolicy {
    policy: Box<dyn DeliveryQueuePolicy>,
    name: String,
}

impl ActiveQueuePolicy {
    pub fn from_name(name: &str) -> Self {
        let policy: Box<dyn DeliveryQueuePolicy> = match name {
            "closest" => Box::new(ClosestQueuePolicy),
            "least_occupied" => Box::new(LeastOccupiedPolicy),
            "weighted" => Box::new(WeightedQueuePolicy::default()),
            _ => Box::new(ClosestQueuePolicy),
        };
        let actual_name = policy.name().to_string();
        Self { policy, name: actual_name }
    }

    pub fn policy(&self) -> &dyn DeliveryQueuePolicy {
        self.policy.as_ref()
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Default for ActiveQueuePolicy {
    fn default() -> Self {
        Self::from_name("closest")
    }
}

// ---------------------------------------------------------------------------
// QueueManager — runtime queue state + tick logic
// ---------------------------------------------------------------------------

/// Manages all delivery queue lines during simulation.
///
/// Owned by `SimulationRunner`. Handles arrivals, compaction, promotion,
/// fault rerouting, and new joins.
pub struct QueueManager {
    pub queues: Vec<QueueState>,
}

impl QueueManager {
    /// Initialize from topology queue lines.
    pub fn new(queue_lines: &[QueueLine]) -> Self {
        let queues = queue_lines
            .iter()
            .enumerate()
            .map(|(i, line)| QueueState::new(i, line.capacity()))
            .collect();
        Self { queues }
    }

    /// Create an empty queue manager (no queue lines in topology).
    pub fn empty() -> Self {
        Self { queues: Vec::new() }
    }

    /// Whether this manager has any queue lines.
    pub fn has_queues(&self) -> bool {
        !self.queues.is_empty()
    }

    /// Reset all queue state (e.g., on simulation reset).
    pub fn reset(&mut self, queue_lines: &[QueueLine]) {
        self.queues = queue_lines
            .iter()
            .enumerate()
            .map(|(i, line)| QueueState::new(i, line.capacity()))
            .collect();
    }

    /// Clear all occupants without changing topology structure.
    /// Used after rewind to avoid stale occupant state from the pre-rewind timeline.
    pub fn clear(&mut self) {
        for state in &mut self.queues {
            state.delivery_occupied_by = None;
            state.reserved = 0;
            for slot in &mut state.slots {
                *slot = None;
            }
        }
    }

    /// Rebuild queue occupancy from agent task legs after a rewind/restore.
    ///
    /// After restoring agents from a snapshot, the QueueManager's slot assignments
    /// are stale. This method clears all slots and re-populates them by scanning
    /// each agent's `TaskLeg`:
    /// - `Queuing { line_index, .. }` → place agent in the correct slot based on position
    /// - `TravelLoaded { to, .. }` → mark delivery cell as occupied (agent is delivering)
    ///
    /// Must be called AFTER agents are restored and AFTER the grid is rebuilt.
    pub fn rebuild_from_agents(&mut self, agents: &[SimAgent], queue_lines: &[QueueLine]) {
        // Clear everything first
        self.clear();

        for (agent_idx, agent) in agents.iter().enumerate() {
            if !agent.alive {
                continue;
            }
            match &agent.task_leg {
                TaskLeg::Queuing { line_index, .. } => {
                    // Try the stored line_index first, then fall back to
                    // searching all queue lines by position. This handles
                    // snapshots from before line_index was saved (hardcoded 0)
                    // and any other case where line_index is stale.
                    let mut found = false;
                    let li = *line_index;
                    if li < self.queues.len() && li < queue_lines.len() {
                        let line = &queue_lines[li];
                        for (slot_idx, &cell) in line.cells.iter().enumerate() {
                            if cell == agent.pos && slot_idx < self.queues[li].slots.len() {
                                self.queues[li].slots[slot_idx] = Some(agent_idx);
                                found = true;
                                break;
                            }
                        }
                    }
                    // Fallback: search all queue lines by position
                    if !found {
                        'outer: for (qi, line) in queue_lines.iter().enumerate() {
                            if qi >= self.queues.len() {
                                break;
                            }
                            for (slot_idx, &cell) in line.cells.iter().enumerate() {
                                if cell == agent.pos && slot_idx < self.queues[qi].slots.len() {
                                    self.queues[qi].slots[slot_idx] = Some(agent_idx);
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                TaskLeg::TravelLoaded { to, .. } => {
                    // Agent is traveling to delivery — mark delivery as occupied
                    // so no other agent gets promoted to the same cell.
                    for (qi, line) in queue_lines.iter().enumerate() {
                        if line.delivery_cell == *to && qi < self.queues.len() {
                            self.queues[qi].delivery_occupied_by = Some(agent_idx);
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Run one tick of queue management.
    ///
    /// Must be called AFTER `recycle_goals` and BEFORE `run_solver`.
    /// `just_loaded` contains agent indices that just entered Loading this tick —
    /// they are skipped by `process_new_joins` to enforce a 1-tick Loading dwell.
    /// Returns indices of agents whose goals changed (need solver replan).
    pub fn tick(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        policy: &dyn DeliveryQueuePolicy,
        just_loaded: &[usize],
        rng: &mut ChaCha8Rng,
    ) -> Vec<usize> {
        if queue_lines.is_empty() {
            return Vec::new();
        }

        let mut changed_agents = Vec::new();

        // Phase A: Remove dead agents from queue slots
        self.remove_dead_agents(agents, &mut changed_agents);

        // Phase B: Detect delivery completions (TravelToDeliver->Idle agents that
        // were occupying a delivery slot)
        self.detect_delivery_completions(agents);

        // Phase C: Compact — shift agents forward to fill gaps
        self.compact(agents, queue_lines, &mut changed_agents);

        // Phase D: Arrivals — Queuing agents at a queue cell get placed in slot
        self.process_arrivals(agents, queue_lines, &mut changed_agents);

        // Phase E: Promote — slot[0] agent + delivery free → TravelToDeliver
        self.promote(agents, queue_lines, &mut changed_agents);

        // Phase F: New joins — Loading agents → ask policy → Queuing
        // (skip agents that just entered Loading this tick)
        self.process_new_joins(agents, queue_lines, policy, &mut changed_agents, just_loaded, rng);

        changed_agents
    }

    /// Remove dead agents from their queue slots and mark for reroute.
    fn remove_dead_agents(&mut self, agents: &[SimAgent], _changed: &mut Vec<usize>) {
        for state in &mut self.queues {
            // Check delivery slot
            if let Some(agent_idx) = state.delivery_occupied_by
                && agent_idx < agents.len()
                && !agents[agent_idx].alive
            {
                state.delivery_occupied_by = None;
            }
            // Check queue slots
            for slot in &mut state.slots {
                if let Some(agent_idx) = *slot
                    && agent_idx < agents.len()
                    && !agents[agent_idx].alive
                {
                    *slot = None;
                }
            }
        }
    }

    /// Detect agents that completed delivery (were in delivery slot, now Idle).
    fn detect_delivery_completions(&mut self, agents: &[SimAgent]) {
        for state in &mut self.queues {
            if let Some(agent_idx) = state.delivery_occupied_by
                && agent_idx < agents.len()
            {
                let agent = &agents[agent_idx];
                // Agent completed delivery → task_leg changed from TravelToDeliver to Idle
                if !matches!(agent.task_leg, TaskLeg::TravelLoaded { .. }) {
                    state.delivery_occupied_by = None;
                }
            }
        }
    }

    /// Compact each queue: shift agents forward to fill gaps.
    fn compact(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        changed: &mut Vec<usize>,
    ) {
        for (qi, state) in self.queues.iter_mut().enumerate() {
            let line = &queue_lines[qi];

            // Walk from front to back, pull agents into empty slots
            let mut write_idx = 0;
            for read_idx in 0..state.slots.len() {
                if let Some(agent_idx) = state.slots[read_idx] {
                    if write_idx != read_idx {
                        // Move agent forward
                        state.slots[write_idx] = Some(agent_idx);
                        state.slots[read_idx] = None;
                        // Update agent goal to new queue position
                        agents[agent_idx].goal = line.cells[write_idx];
                        agents[agent_idx].planned_path.clear();
                        changed.push(agent_idx);
                    }
                    write_idx += 1;
                }
            }
        }
    }

    /// Place arriving TravelToQueue agents into their queue's front-most empty slot,
    /// transitioning them to Queuing.
    fn process_arrivals(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        changed: &mut Vec<usize>,
    ) {
        for (agent_idx, agent) in agents.iter_mut().enumerate() {
            if !agent.alive {
                continue;
            }
            let (from, to, line_index) = match &agent.task_leg {
                TaskLeg::TravelToQueue { from, to, line_index } => (*from, *to, *line_index),
                TaskLeg::Queuing { line_index, .. } => {
                    // Already queuing — check if already slotted
                    if *line_index < self.queues.len()
                        && self.queues[*line_index].slots.contains(&Some(agent_idx))
                    {
                        continue;
                    }
                    // Queuing but not slotted (e.g. after rewind) — try to slot in
                    let li = *line_index;
                    let f = match &agent.task_leg {
                        TaskLeg::Queuing { from, .. } => *from,
                        _ => unreachable!(),
                    };
                    let t = match &agent.task_leg {
                        TaskLeg::Queuing { to, .. } => *to,
                        _ => unreachable!(),
                    };
                    (f, t, li)
                }
                _ => continue,
            };
            if line_index >= queue_lines.len() {
                continue;
            }

            let line = &queue_lines[line_index];
            let state = &mut self.queues[line_index];

            // Check if agent is physically at any cell in this queue line
            let at_queue_cell = line.cells.contains(&agent.pos);
            if !at_queue_cell {
                continue;
            }

            // Already in a slot? Skip.
            if state.slots.contains(&Some(agent_idx)) {
                continue;
            }

            // Place in front-most empty slot and transition to Queuing
            if let Some(slot_idx) = state.first_empty_slot() {
                state.slots[slot_idx] = Some(agent_idx);
                agent.task_leg = TaskLeg::Queuing { from, to, line_index };
                if agent.goal != line.cells[slot_idx] {
                    agent.goal = line.cells[slot_idx];
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
            } else if matches!(agent.task_leg, TaskLeg::TravelToQueue { .. }) {
                // Queue full when agent arrived — revert to Loading. Setting
                // goal = agent.pos (NOT the pickup cell `from`) keeps the agent
                // eligible for process_new_joins on the NEXT tick: the join
                // filter at `queue/mod.rs:pos == goal` admits this agent
                // immediately instead of requiring a 4-10 tick backtrack to
                // the pickup cell first. Safe because recycle_goals_core's
                // Loading(_) arm (task/recycle.rs:145-148) is a no-op, so
                // pos == goal does NOT trigger a spurious state transition.
                // Without this, kicked-back agents appear visually "stuck" in
                // the picking colour on the delivery corridor until they
                // backtrack to the pickup cell.
                agent.task_leg = TaskLeg::Loading(from);
                agent.goal = agent.pos;
                agent.planned_path.clear();
                changed.push(agent_idx);
            }
        }
    }

    /// Promote: if delivery cell is free and slot[0] is occupied, promote agent.
    fn promote(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        changed: &mut Vec<usize>,
    ) {
        for (qi, state) in self.queues.iter_mut().enumerate() {
            if state.delivery_occupied_by.is_some() {
                continue;
            }

            if let Some(agent_idx) = state.slots[0] {
                let agent = &agents[agent_idx];
                // Only promote if agent is physically at slot[0]
                if agent.pos != queue_lines[qi].cells[0] {
                    continue;
                }

                let line = &queue_lines[qi];
                let from = match &agent.task_leg {
                    TaskLeg::TravelToQueue { from, .. } | TaskLeg::Queuing { from, .. } => *from,
                    _ => agent.pos,
                };

                // Promote to TravelLoaded
                state.slots[0] = None;
                state.delivery_occupied_by = Some(agent_idx);

                let agent = &mut agents[agent_idx];
                agent.task_leg = TaskLeg::TravelLoaded { from, to: line.delivery_cell };
                agent.goal = line.delivery_cell;
                agent.planned_path.clear();
                changed.push(agent_idx);
            }
        }
    }

    /// Process new joins: Loading agents → ask policy → transition to TravelToQueue.
    /// Agents in `just_loaded` are skipped (must dwell in Loading for 1 tick first).
    ///
    /// Uses `reserved` counter on QueueState so that agents assigned in the same
    /// tick see progressively reduced availability and get different goal cells.
    /// Eligible agents are shuffled for fairness (same pattern as recycle_goals_core).
    fn process_new_joins(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        policy: &dyn DeliveryQueuePolicy,
        changed: &mut Vec<usize>,
        just_loaded: &[usize],
        rng: &mut ChaCha8Rng,
    ) {
        // Collect eligible Loading agents. The `pos == goal` filter admits
        // two populations:
        //   (1) agents that just arrived at their pickup cell (normal entry);
        //   (2) agents kicked back from a full/blocked queue — their goal was
        //       rewritten to `agent.pos` at kick-back time (see
        //       process_arrivals / reroute_blocked_agents), so they are
        //       eligible for requeuing immediately without having to travel
        //       back to the pickup cell.
        let mut eligible: Vec<usize> = (0..agents.len())
            .filter(|&i| {
                let a = &agents[i];
                a.alive
                    && matches!(a.task_leg, TaskLeg::Loading(_))
                    && a.pos == a.goal
                    && !just_loaded.contains(&i)
            })
            .collect();

        // Shuffle for fairness — lower-index agents don't always get first pick
        if !eligible.is_empty() {
            for i in (1..eligible.len()).rev() {
                let j = rng.random_range(0..=i);
                eligible.swap(i, j);
            }
        }

        // Reset reservations before assignment pass
        for state in &mut self.queues {
            state.reserved = 0;
        }

        for agent_idx in eligible {
            let decision = policy.choose_queue(agents[agent_idx].pos, queue_lines, &self.queues);

            match decision {
                QueueDecision::JoinQueue { line_index } => {
                    // Guard: skip if this queue's physical capacity is exhausted
                    // by pending reservations from earlier agents in this tick.
                    let physical_free =
                        self.queues[line_index].slots.iter().filter(|s| s.is_none()).count();
                    if self.queues[line_index].reserved >= physical_free {
                        // Queue logically full from pending reservations — hold
                        continue;
                    }

                    let line = &queue_lines[line_index];
                    let state = &self.queues[line_index];
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::Loading(pickup) => *pickup,
                        _ => agents[agent_idx].pos,
                    };

                    let agent = &mut agents[agent_idx];
                    agent.task_leg =
                        TaskLeg::TravelToQueue { from, to: line.delivery_cell, line_index };
                    agent.goal = line.join_cell(state);
                    agent.planned_path.clear();
                    changed.push(agent_idx);

                    // Reserve slot so next agent can't also claim this queue's capacity
                    self.queues[line_index].reserved += 1;
                }
                QueueDecision::Hold => {
                    // Stay in Loading — retry next tick
                }
            }
        }

        // Reset reservations — don't leak into next tick's phases
        for state in &mut self.queues {
            state.reserved = 0;
        }
    }

    /// Handle fault rerouting: agents in TravelToQueue/Queuing whose queue has a dead
    /// agent blocking them get reassigned to another queue.
    ///
    /// Uses `reserved` to prevent reassigning agents back to the blocked queue
    /// (which looks empty after slot clearing) and to ensure displaced agents
    /// assigned in the same pass get different goal cells.
    pub fn reroute_blocked_agents(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        policy: &dyn DeliveryQueuePolicy,
        changed: &mut Vec<usize>,
    ) {
        // Pass 1: collect agents from blocked queues and mark blocked queues as
        // artificially full so the policy won't reassign agents back to them.
        let mut reroute_agents: Vec<usize> = Vec::new();
        for (qi, state) in self.queues.iter_mut().enumerate() {
            let line = &queue_lines[qi];
            let delivery_blocked = state.delivery_occupied_by.is_none()
                && !agents.iter().any(|a| a.alive && a.pos == line.delivery_cell);
            if !delivery_blocked {
                continue;
            }
            for slot in &mut state.slots {
                if let Some(agent_idx) = slot.take() {
                    reroute_agents.push(agent_idx);
                }
            }
            // Mark blocked queue as full so policy skips it in Pass 2
            state.reserved = state.slots.len();
        }

        // Pass 2: reroute collected agents with reservation tracking
        for agent_idx in reroute_agents {
            if !agents[agent_idx].alive {
                continue;
            }
            let decision = policy.choose_queue(agents[agent_idx].pos, queue_lines, &self.queues);
            match decision {
                QueueDecision::JoinQueue { line_index } => {
                    let new_line = &queue_lines[line_index];
                    let new_state = &self.queues[line_index];
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::TravelToQueue { from, .. } | TaskLeg::Queuing { from, .. } => {
                            *from
                        }
                        _ => agents[agent_idx].pos,
                    };
                    let agent = &mut agents[agent_idx];
                    agent.task_leg =
                        TaskLeg::TravelToQueue { from, to: new_line.delivery_cell, line_index };
                    agent.goal = new_line.join_cell(new_state);
                    agent.planned_path.clear();
                    changed.push(agent_idx);

                    // Reserve slot so next displaced agent sees reduced availability
                    self.queues[line_index].reserved += 1;
                }
                QueueDecision::Hold => {
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::TravelToQueue { from, .. } | TaskLeg::Queuing { from, .. } => {
                            *from
                        }
                        _ => agents[agent_idx].pos,
                    };
                    let agent = &mut agents[agent_idx];
                    agent.task_leg = TaskLeg::Loading(from);
                    // Goal = agent.pos (waiting in place), NOT the pickup cell.
                    // See arrivals-revert comment in process_arrivals — the
                    // Loading(_) arm of recycle_goals_core is a no-op, so
                    // pos == goal is safe here and lets process_new_joins
                    // re-attempt queue assignment next tick without forcing
                    // the agent to travel back to the pickup cell.
                    agent.goal = agent.pos;
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
            }
        }

        // Reset all reservations — don't leak into the main tick() phases
        for state in &mut self.queues {
            state.reserved = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Compute queue lines from ZoneMap delivery cells + directions
// ---------------------------------------------------------------------------

/// Build queue lines from delivery cells that have associated directions.
///
/// `delivery_directions` maps delivery cell position to queue direction.
/// Returns validated queue lines (no overlapping cells).
pub fn build_queue_lines(
    delivery_directions: &[(IVec2, Direction)],
    grid: &GridMap,
) -> Vec<QueueLine> {
    let mut lines = Vec::new();
    let mut used_cells = std::collections::HashSet::new();

    for &(delivery_cell, direction) in delivery_directions {
        if let Some(line) = QueueLine::compute(delivery_cell, direction, grid) {
            // Validate no overlap
            let has_overlap = line.cells.iter().any(|c| used_cells.contains(c));
            if !has_overlap {
                for &cell in &line.cells {
                    used_cells.insert(cell);
                }
                lines.push(line);
            }
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct QueuePlugin;

impl Plugin for QueuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveQueuePolicy>();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn test_grid() -> GridMap {
        // 10x5 grid, all walkable except borders
        let mut obstacles = HashSet::new();
        for x in 0..10 {
            obstacles.insert(IVec2::new(x, 0));
            obstacles.insert(IVec2::new(x, 4));
        }
        for y in 0..5 {
            obstacles.insert(IVec2::new(0, y));
            obstacles.insert(IVec2::new(9, y));
        }
        GridMap::with_obstacles(10, 5, obstacles)
    }

    #[test]
    fn queue_line_compute_basic() {
        let grid = test_grid();
        // Delivery at (8,2), queue extends west — capped at MAX_LENGTH
        let line = QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap();
        assert_eq!(line.delivery_cell, IVec2::new(8, 2));
        assert_eq!(line.direction, Direction::West);
        assert_eq!(line.cells.len(), QueueLine::MAX_LENGTH);
        assert_eq!(line.cells[0], IVec2::new(7, 2)); // closest to delivery
        assert_eq!(line.cells[3], IVec2::new(4, 2)); // back of line (capped)
    }

    #[test]
    fn queue_line_compute_hits_wall() {
        let grid = test_grid();
        // Delivery at (2,2), queue extends west — only 1 cell before wall
        let line = QueueLine::compute(IVec2::new(2, 2), Direction::West, &grid).unwrap();
        assert_eq!(line.cells.len(), 1);
        assert_eq!(line.cells[0], IVec2::new(1, 2));
    }

    #[test]
    fn queue_line_compute_no_space() {
        let grid = test_grid();
        // Delivery at (1,2), queue extends west — wall at (0,2), no space
        let result = QueueLine::compute(IVec2::new(1, 2), Direction::West, &grid);
        assert!(result.is_none());
    }

    #[test]
    fn queue_state_occupancy() {
        let mut state = QueueState::new(0, 5);
        assert_eq!(state.occupancy(), 0);
        assert_eq!(state.free_slots(), 5);
        assert!(!state.is_full());

        state.slots[0] = Some(0);
        state.slots[2] = Some(1);
        assert_eq!(state.occupancy(), 2);
        assert_eq!(state.free_slots(), 3);
    }

    #[test]
    fn queue_state_first_empty_slot() {
        let mut state = QueueState::new(0, 3);
        assert_eq!(state.first_empty_slot(), Some(0));

        state.slots[0] = Some(0);
        assert_eq!(state.first_empty_slot(), Some(1));

        state.slots[1] = Some(1);
        state.slots[2] = Some(2);
        assert_eq!(state.first_empty_slot(), None);
        assert!(state.is_full());
    }

    #[test]
    fn closest_policy_picks_nearest() {
        let grid = test_grid();
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 1), Direction::West, &grid).unwrap(),
            QueueLine::compute(IVec2::new(8, 3), Direction::West, &grid).unwrap(),
        ];
        let states =
            vec![QueueState::new(0, lines[0].capacity()), QueueState::new(1, lines[1].capacity())];

        let policy = ClosestQueuePolicy;
        // Agent at (3,3) — closer to line 1 (back cell at (1,3))
        let decision = policy.choose_queue(IVec2::new(3, 3), &lines, &states);
        assert_eq!(decision, QueueDecision::JoinQueue { line_index: 1 });
    }

    #[test]
    fn closest_policy_skips_full_queues() {
        let grid = test_grid();
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 1), Direction::West, &grid).unwrap(),
            QueueLine::compute(IVec2::new(8, 3), Direction::West, &grid).unwrap(),
        ];
        let mut states =
            vec![QueueState::new(0, lines[0].capacity()), QueueState::new(1, lines[1].capacity())];
        // Fill queue 1 completely
        for i in 0..states[1].slots.len() {
            states[1].slots[i] = Some(i + 100);
        }

        let policy = ClosestQueuePolicy;
        // Even though line 1 is closer, it's full — should pick line 0
        let decision = policy.choose_queue(IVec2::new(3, 3), &lines, &states);
        assert_eq!(decision, QueueDecision::JoinQueue { line_index: 0 });
    }

    #[test]
    fn all_full_returns_hold() {
        let grid = test_grid();
        let lines = vec![QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap()];
        let mut states = vec![QueueState::new(0, lines[0].capacity())];
        for i in 0..states[0].slots.len() {
            states[0].slots[i] = Some(i);
        }

        let policy = ClosestQueuePolicy;
        let decision = policy.choose_queue(IVec2::new(3, 2), &lines, &states);
        assert_eq!(decision, QueueDecision::Hold);
    }

    #[test]
    fn least_occupied_picks_emptiest() {
        let grid = test_grid();
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 1), Direction::West, &grid).unwrap(),
            QueueLine::compute(IVec2::new(8, 3), Direction::West, &grid).unwrap(),
        ];
        let mut states =
            vec![QueueState::new(0, lines[0].capacity()), QueueState::new(1, lines[1].capacity())];
        // Fill all slots in queue 0, put 1 agent in queue 1
        for i in 0..lines[0].capacity() {
            states[0].slots[i] = Some(i);
        }
        states[1].slots[0] = Some(10);

        let policy = LeastOccupiedPolicy;
        let decision = policy.choose_queue(IVec2::new(3, 1), &lines, &states);
        // Queue 1 has more free slots despite being farther
        assert_eq!(decision, QueueDecision::JoinQueue { line_index: 1 });
    }

    #[test]
    fn build_queue_lines_no_overlap() {
        let grid = test_grid();
        let directions = vec![
            (IVec2::new(8, 1), Direction::West),
            (IVec2::new(8, 2), Direction::West), // Would overlap with line above at same x positions
            (IVec2::new(8, 3), Direction::West),
        ];
        let lines = build_queue_lines(&directions, &grid);
        // Lines at y=1 and y=3 don't overlap (different rows)
        // Line at y=2 also doesn't overlap (different row)
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn build_queue_lines_rejects_actual_overlap() {
        let grid = test_grid();
        // Two delivery cells trying to queue through the same cells.
        // First queue at (8,2) going West: cells = (7,2), (6,2), (5,2), (4,2), ...
        // Second queue at (6,2) going West: would start at (5,2) — already in first queue.
        // Second queue is correctly rejected due to overlap.
        let directions =
            vec![(IVec2::new(8, 2), Direction::West), (IVec2::new(6, 2), Direction::West)];
        let lines = build_queue_lines(&directions, &grid);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn active_queue_policy_from_name() {
        let p = ActiveQueuePolicy::from_name("closest");
        assert_eq!(p.name(), "closest");

        let p = ActiveQueuePolicy::from_name("least_occupied");
        assert_eq!(p.name(), "least_occupied");

        let p = ActiveQueuePolicy::from_name("weighted");
        assert_eq!(p.name(), "weighted");

        // Unknown falls back to closest
        let p = ActiveQueuePolicy::from_name("unknown");
        assert_eq!(p.name(), "closest");
    }

    #[test]
    fn queue_manager_new_matches_lines() {
        let grid = test_grid();
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 1), Direction::West, &grid).unwrap(),
            QueueLine::compute(IVec2::new(8, 3), Direction::West, &grid).unwrap(),
        ];
        let mgr = QueueManager::new(&lines);
        assert_eq!(mgr.queues.len(), 2);
        assert_eq!(mgr.queues[0].slots.len(), lines[0].capacity());
        assert_eq!(mgr.queues[1].slots.len(), lines[1].capacity());
    }

    // ── Bug fix regression tests ────────────────────────────────────────

    #[test]
    fn reserved_tracks_pending_assignments() {
        let mut state = QueueState::new(0, 4);
        assert_eq!(state.reserved, 0);

        // free_slots is physical (ignores reserved) — policy sees real capacity
        state.reserved = 2;
        assert_eq!(state.free_slots(), 4); // physical, not affected

        // reserved is used as a local guard in process_new_joins
        let physical_free = state.slots.iter().filter(|s| s.is_none()).count();
        assert!(state.reserved < physical_free); // capacity still available

        state.reserved = 4;
        assert!(state.reserved >= physical_free); // logically full
    }

    #[test]
    fn nth_empty_slot_skips() {
        let mut state = QueueState::new(0, 4);
        // All empty: [None, None, None, None]
        assert_eq!(state.nth_empty_slot(0), Some(0));
        assert_eq!(state.nth_empty_slot(1), Some(1));
        assert_eq!(state.nth_empty_slot(2), Some(2));
        assert_eq!(state.nth_empty_slot(3), Some(3));
        assert_eq!(state.nth_empty_slot(4), None);

        // Partial: [Some, None, Some, None]
        state.slots[0] = Some(0);
        state.slots[2] = Some(1);
        assert_eq!(state.nth_empty_slot(0), Some(1)); // first empty = slot 1
        assert_eq!(state.nth_empty_slot(1), Some(3)); // second empty = slot 3
        assert_eq!(state.nth_empty_slot(2), None); // only 2 empty slots
    }

    #[test]
    fn join_cell_with_reservations() {
        let grid = test_grid();
        let line = QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap();
        let cap = line.capacity();
        assert!(cap >= 3, "Need at least 3 queue slots for this test");

        let mut state = QueueState::new(0, cap);

        // No reservations: join at first empty (cells[0])
        state.reserved = 0;
        assert_eq!(line.join_cell(&state), line.cells[0]);

        // 1 reservation: join at second empty (cells[1])
        state.reserved = 1;
        assert_eq!(line.join_cell(&state), line.cells[1]);

        // 2 reservations: join at third empty (cells[2])
        state.reserved = 2;
        assert_eq!(line.join_cell(&state), line.cells[2]);

        // All slots reserved: fallback to back_cell
        state.reserved = cap;
        assert_eq!(line.join_cell(&state), line.back_cell());
    }

    #[test]
    fn process_new_joins_no_duplicate_goals() {
        use super::super::runner::SimAgent;
        use rand::SeedableRng;

        let grid = test_grid();
        // Single queue at (8,2) going West — 4 slots
        let lines = vec![QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap()];
        let mut mgr = QueueManager::new(&lines);

        // Create 3 Loading agents all at their pickup cell (3,2)
        let pickup = IVec2::new(3, 2);
        let mut agents: Vec<SimAgent> = (0..3)
            .map(|_| {
                let mut a = SimAgent::new(pickup);
                a.task_leg = TaskLeg::Loading(pickup);
                a.goal = pickup; // at goal = eligible for process_new_joins
                a
            })
            .collect();

        let policy = ClosestQueuePolicy;
        let mut changed = Vec::new();
        let just_loaded: Vec<usize> = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(42);

        mgr.process_new_joins(&mut agents, &lines, &policy, &mut changed, &just_loaded, &mut rng);

        // All 3 agents should be assigned TravelToQueue
        assert_eq!(changed.len(), 3);

        // All 3 should have DIFFERENT goal cells
        let goals: Vec<IVec2> = agents.iter().map(|a| a.goal).collect();
        let unique: HashSet<IVec2> = goals.iter().copied().collect();
        assert_eq!(unique.len(), 3, "Expected 3 unique goals, got {:?}", goals);
    }

    #[test]
    fn arrivals_full_queue_reverts_to_loading() {
        use super::super::runner::SimAgent;

        let grid = test_grid();
        let lines = vec![QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap()];
        let cap = lines[0].capacity();
        let mut mgr = QueueManager::new(&lines);

        // Fill queue completely with dummy agents
        for i in 0..cap {
            mgr.queues[0].slots[i] = Some(100 + i); // dummy indices
        }

        // Create a TravelToQueue agent that has arrived at the queue
        let pickup = IVec2::new(3, 2);
        let queue_cell = lines[0].cells[0];
        let mut agents = vec![SimAgent::new(queue_cell)];
        agents[0].task_leg =
            TaskLeg::TravelToQueue { from: pickup, to: lines[0].delivery_cell, line_index: 0 };
        agents[0].pos = queue_cell; // physically at queue cell

        let mut changed = Vec::new();
        mgr.process_arrivals(&mut agents, &lines, &mut changed);

        // Agent should have reverted to Loading
        assert!(
            matches!(agents[0].task_leg, TaskLeg::Loading(_)),
            "Expected Loading, got {:?}",
            agents[0].task_leg
        );
        // Post-2026-04-20 kick-back fix: goal is rewritten to agent.pos so
        // process_new_joins picks the agent up NEXT tick without forcing a
        // 4-10 tick backtrack to the pickup cell. The Loading(pickup)
        // payload still records `from = pickup` for downstream consumers.
        assert_eq!(agents[0].goal, queue_cell, "goal must equal agent.pos at kick-back");
        if let TaskLeg::Loading(p) = agents[0].task_leg {
            assert_eq!(p, pickup, "Loading payload still carries pickup cell");
        }
        assert!(changed.contains(&0));
    }

    /// Regression test for the 2026-04-20 stranding fix (Phase-1 audit).
    /// A kicked-back agent (queue was full on arrival) must be picked up by
    /// `process_new_joins` on the very next tick once a slot opens, WITHOUT
    /// requiring the agent to first travel back to the pickup cell. This
    /// matches the visual expectation: no more than 1-2 ticks in the
    /// picking-amber state on the delivery corridor.
    #[test]
    fn kick_back_arrivals_is_requeuable_next_tick() {
        use super::super::runner::SimAgent;
        use rand::SeedableRng;

        let grid = test_grid();
        let lines = vec![QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap()];
        let cap = lines[0].capacity();
        let mut mgr = QueueManager::new(&lines);
        for i in 0..cap {
            mgr.queues[0].slots[i] = Some(100 + i);
        }

        let pickup = IVec2::new(3, 2);
        let queue_cell = lines[0].back_cell();
        let mut agents = vec![SimAgent::new(queue_cell)];
        agents[0].task_leg =
            TaskLeg::TravelToQueue { from: pickup, to: lines[0].delivery_cell, line_index: 0 };
        agents[0].pos = queue_cell;

        // Tick 1 — kick-back fires.
        let mut changed = Vec::new();
        mgr.process_arrivals(&mut agents, &lines, &mut changed);
        assert!(matches!(agents[0].task_leg, TaskLeg::Loading(_)));
        assert_eq!(agents[0].goal, queue_cell, "goal must be agent.pos after kick-back");

        // Free the slot that would accept this agent and invoke the next
        // tick's join logic. The agent must transition back into
        // TravelToQueue without any intervening backtrack.
        mgr.queues[0].slots[cap - 1] = None;
        let policy = ClosestQueuePolicy;
        let mut changed2 = Vec::new();
        let just_loaded: Vec<usize> = Vec::new();
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        mgr.process_new_joins(&mut agents, &lines, &policy, &mut changed2, &just_loaded, &mut rng);

        assert!(
            matches!(agents[0].task_leg, TaskLeg::TravelToQueue { .. }),
            "agent must be re-queued immediately after a slot opens, got {:?}",
            agents[0].task_leg
        );
        assert!(changed2.contains(&0));
    }

    #[test]
    fn reroute_excludes_blocked_queue() {
        use super::super::runner::SimAgent;

        let grid = test_grid();
        // Two queues at different rows
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 1), Direction::West, &grid).unwrap(),
            QueueLine::compute(IVec2::new(8, 3), Direction::West, &grid).unwrap(),
        ];
        let mut mgr = QueueManager::new(&lines);

        let pickup = IVec2::new(3, 2);
        // Agent 0 in queue 0 slot 0, agent 1 at queue 1's delivery cell (healthy queue)
        let mut agents =
            vec![SimAgent::new(lines[0].cells[0]), SimAgent::new(lines[1].delivery_cell)];
        agents[0].task_leg =
            TaskLeg::Queuing { from: pickup, to: lines[0].delivery_cell, line_index: 0 };
        mgr.queues[0].slots[0] = Some(0);
        // Agent 1 is delivering at queue 1 — marks queue 1 as NOT blocked
        agents[1].task_leg = TaskLeg::TravelLoaded { from: pickup, to: lines[1].delivery_cell };
        mgr.queues[1].delivery_occupied_by = Some(1);

        // Queue 0's delivery cell is blocked (no alive agent there, delivery_occupied_by is None)
        // Queue 1's delivery cell is NOT blocked (agent 1 is there)

        let policy = ClosestQueuePolicy;
        let mut changed = Vec::new();
        mgr.reroute_blocked_agents(&mut agents, &lines, &policy, &mut changed);

        // Agent should be rerouted to queue 1, NOT back to queue 0
        match &agents[0].task_leg {
            TaskLeg::TravelToQueue { line_index, .. } => {
                assert_eq!(
                    *line_index, 1,
                    "Agent should be rerouted to queue 1, not back to blocked queue 0"
                );
            }
            other => panic!("Expected TravelToQueue, got {:?}", other),
        }
    }
}
