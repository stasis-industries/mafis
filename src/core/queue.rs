//! Delivery queue line system — modular queuing for delivery zones.
//!
//! Each delivery cell has a directed queue line (sequence of walkable cells
//! extending in one direction until a wall). Agents join the back of a queue,
//! shuffle forward each tick, and get promoted to delivery when the cell is free.
//!
//! Queue assignment policy (`DeliveryQueuePolicy`) is a swappable research
//! variable — parallel to `TaskScheduler` and `LifelongSolver`.

use bevy::prelude::*;

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

        Some(Self {
            delivery_cell,
            direction,
            cells,
        })
    }

    /// Back-of-line cell (where new agents aim to join).
    pub fn back_cell(&self) -> IVec2 {
        *self.cells.last().unwrap_or(&self.delivery_cell)
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
}

impl QueueState {
    pub fn new(line_index: usize, capacity: usize) -> Self {
        Self {
            line_index,
            slots: vec![None; capacity],
            delivery_occupied_by: None,
        }
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

    /// Find the front-most empty slot index.
    fn first_empty_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
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
// DeliveryQueuePolicy trait
// ---------------------------------------------------------------------------

/// Policy for choosing which delivery queue an agent should join.
///
/// This is a research variable — different policies produce different
/// queuing behavior and resilience characteristics.
pub trait DeliveryQueuePolicy: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Decide which queue an agent should join.
    fn choose_queue(
        &self,
        agent_pos: IVec2,
        queue_lines: &[QueueLine],
        queue_states: &[QueueState],
    ) -> QueueDecision;
}

// ---------------------------------------------------------------------------
// Policy implementations
// ---------------------------------------------------------------------------

/// Pick the nearest non-full queue (Manhattan distance to back-of-line cell).
pub struct ClosestQueuePolicy;

impl DeliveryQueuePolicy for ClosestQueuePolicy {
    fn name(&self) -> &str {
        "closest"
    }

    fn choose_queue(
        &self,
        agent_pos: IVec2,
        queue_lines: &[QueueLine],
        queue_states: &[QueueState],
    ) -> QueueDecision {
        let mut best: Option<(usize, i32)> = None;

        for (i, (line, state)) in queue_lines.iter().zip(queue_states.iter()).enumerate() {
            if state.is_full() {
                continue;
            }
            let dist = (agent_pos.x - line.back_cell().x).abs()
                + (agent_pos.y - line.back_cell().y).abs();
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((i, dist));
            }
        }

        match best {
            Some((idx, _)) => QueueDecision::JoinQueue { line_index: idx },
            None => QueueDecision::Hold,
        }
    }
}

/// Pick the queue with the most empty slots. Tie-break by distance.
pub struct LeastOccupiedPolicy;

impl DeliveryQueuePolicy for LeastOccupiedPolicy {
    fn name(&self) -> &str {
        "least_occupied"
    }

    fn choose_queue(
        &self,
        agent_pos: IVec2,
        queue_lines: &[QueueLine],
        queue_states: &[QueueState],
    ) -> QueueDecision {
        let mut best: Option<(usize, usize, i32)> = None; // (index, free_slots, distance)

        for (i, (line, state)) in queue_lines.iter().zip(queue_states.iter()).enumerate() {
            if state.is_full() {
                continue;
            }
            let free = state.free_slots();
            let dist = (agent_pos.x - line.back_cell().x).abs()
                + (agent_pos.y - line.back_cell().y).abs();
            let is_better = match best {
                None => true,
                Some((_, best_free, best_dist)) => {
                    free > best_free || (free == best_free && dist < best_dist)
                }
            };
            if is_better {
                best = Some((i, free, dist));
            }
        }

        match best {
            Some((idx, _, _)) => QueueDecision::JoinQueue { line_index: idx },
            None => QueueDecision::Hold,
        }
    }
}

/// Weighted combination of distance and occupancy. Lower score wins.
pub struct WeightedQueuePolicy {
    pub distance_weight: f32,
    pub occupancy_weight: f32,
}

impl Default for WeightedQueuePolicy {
    fn default() -> Self {
        Self {
            distance_weight: 0.5,
            occupancy_weight: 0.5,
        }
    }
}

impl DeliveryQueuePolicy for WeightedQueuePolicy {
    fn name(&self) -> &str {
        "weighted"
    }

    fn choose_queue(
        &self,
        agent_pos: IVec2,
        queue_lines: &[QueueLine],
        queue_states: &[QueueState],
    ) -> QueueDecision {
        if queue_lines.is_empty() {
            return QueueDecision::Hold;
        }

        // Compute max distance and max occupancy for normalization
        let mut max_dist: f32 = 1.0;
        let mut max_occ: f32 = 1.0;
        for (line, state) in queue_lines.iter().zip(queue_states.iter()) {
            let dist = ((agent_pos.x - line.back_cell().x).abs()
                + (agent_pos.y - line.back_cell().y).abs()) as f32;
            let occ = state.occupancy() as f32;
            if dist > max_dist {
                max_dist = dist;
            }
            if occ > max_occ {
                max_occ = occ;
            }
        }

        let mut best: Option<(usize, f32)> = None;

        for (i, (line, state)) in queue_lines.iter().zip(queue_states.iter()).enumerate() {
            if state.is_full() {
                continue;
            }
            let dist = ((agent_pos.x - line.back_cell().x).abs()
                + (agent_pos.y - line.back_cell().y).abs()) as f32;
            let occ = state.occupancy() as f32;

            let score = self.distance_weight * (dist / max_dist)
                + self.occupancy_weight * (occ / max_occ);

            if best.is_none() || score < best.unwrap().1 {
                best = Some((i, score));
            }
        }

        match best {
            Some((idx, _)) => QueueDecision::JoinQueue { line_index: idx },
            None => QueueDecision::Hold,
        }
    }
}

// ---------------------------------------------------------------------------
// ActiveQueuePolicy resource
// ---------------------------------------------------------------------------

pub const QUEUE_POLICY_NAMES: &[(&str, &str)] = &[
    ("closest", "Closest"),
    ("least_occupied", "Least Occupied"),
    ("weighted", "Weighted"),
];

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
        Self {
            policy,
            name: actual_name,
        }
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
        Self {
            queues: Vec::new(),
        }
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
    pub fn rebuild_from_agents(
        &mut self,
        agents: &[SimAgent],
        queue_lines: &[QueueLine],
    ) {
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
                            if qi >= self.queues.len() { break; }
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
        self.process_new_joins(agents, queue_lines, policy, &mut changed_agents, just_loaded);

        changed_agents
    }

    /// Remove dead agents from their queue slots and mark for reroute.
    fn remove_dead_agents(
        &mut self,
        agents: &[SimAgent],
        _changed: &mut Vec<usize>,
    ) {
        for state in &mut self.queues {
            // Check delivery slot
            if let Some(agent_idx) = state.delivery_occupied_by
                && agent_idx < agents.len() && !agents[agent_idx].alive {
                    state.delivery_occupied_by = None;
                }
            // Check queue slots
            for slot in &mut state.slots {
                if let Some(agent_idx) = *slot
                    && agent_idx < agents.len() && !agents[agent_idx].alive {
                        *slot = None;
                    }
            }
        }
    }

    /// Detect agents that completed delivery (were in delivery slot, now Idle).
    fn detect_delivery_completions(&mut self, agents: &[SimAgent]) {
        for state in &mut self.queues {
            if let Some(agent_idx) = state.delivery_occupied_by
                && agent_idx < agents.len() {
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
                agent.task_leg = TaskLeg::Queuing {
                    from,
                    to,
                    line_index,
                };
                if agent.goal != line.cells[slot_idx] {
                    agent.goal = line.cells[slot_idx];
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
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
                    TaskLeg::TravelToQueue { from, .. }
                    | TaskLeg::Queuing { from, .. } => *from,
                    _ => agent.pos,
                };

                // Promote to TravelLoaded
                state.slots[0] = None;
                state.delivery_occupied_by = Some(agent_idx);

                let agent = &mut agents[agent_idx];
                agent.task_leg = TaskLeg::TravelLoaded {
                    from,
                    to: line.delivery_cell,
                };
                agent.goal = line.delivery_cell;
                agent.planned_path.clear();
                changed.push(agent_idx);
            }
        }
    }

    /// Process new joins: Loading agents → ask policy → transition to TravelToQueue.
    /// Agents in `just_loaded` are skipped (must dwell in Loading for 1 tick first).
    fn process_new_joins(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        policy: &dyn DeliveryQueuePolicy,
        changed: &mut Vec<usize>,
        just_loaded: &[usize],
    ) {
        #[allow(clippy::needless_range_loop)]
        for agent_idx in 0..agents.len() {
            let agent = &agents[agent_idx];
            if !agent.alive {
                continue;
            }
            // Only process Loading agents that are at their goal (at pickup cell)
            if !matches!(agent.task_leg, TaskLeg::Loading(_)) {
                continue;
            }
            if agent.pos != agent.goal {
                continue;
            }
            // Skip agents that just entered Loading this tick
            if just_loaded.contains(&agent_idx) {
                continue;
            }

            let decision = policy.choose_queue(agent.pos, queue_lines, &self.queues);

            match decision {
                QueueDecision::JoinQueue { line_index } => {
                    let line = &queue_lines[line_index];
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::Loading(pickup) => *pickup,
                        _ => agents[agent_idx].pos,
                    };

                    let agent = &mut agents[agent_idx];
                    agent.task_leg = TaskLeg::TravelToQueue {
                        from,
                        to: line.delivery_cell,
                        line_index,
                    };
                    agent.goal = line.back_cell();
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
                QueueDecision::Hold => {
                    // Stay in Loading — retry next tick
                }
            }
        }
    }

    /// Handle fault rerouting: agents in TravelToQueue/Queuing whose queue has a dead
    /// agent blocking them get reassigned to another queue.
    pub fn reroute_blocked_agents(
        &mut self,
        agents: &mut [SimAgent],
        queue_lines: &[QueueLine],
        policy: &dyn DeliveryQueuePolicy,
        changed: &mut Vec<usize>,
    ) {
        // Find agents in TravelToQueue/Queuing that need rerouting (their queue
        // has a dead agent blocking the delivery cell).
        // After remove_dead_agents + compact, gaps are filled automatically.
        // Explicit rerouting is only needed if the delivery cell itself is
        // permanently blocked (dead agent became obstacle there).
        // Pass 1: collect agents from blocked queues (mutable borrow of self.queues)
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
        }

        // Pass 2: reroute collected agents (immutable borrow of self.queues for policy)
        for agent_idx in reroute_agents {
            if !agents[agent_idx].alive {
                continue;
            }
            let decision = policy.choose_queue(agents[agent_idx].pos, queue_lines, &self.queues);
            match decision {
                QueueDecision::JoinQueue { line_index } => {
                    let new_line = &queue_lines[line_index];
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::TravelToQueue { from, .. }
                        | TaskLeg::Queuing { from, .. } => *from,
                        _ => agents[agent_idx].pos,
                    };
                    let agent = &mut agents[agent_idx];
                    agent.task_leg = TaskLeg::TravelToQueue {
                        from,
                        to: new_line.delivery_cell,
                        line_index,
                    };
                    agent.goal = new_line.back_cell();
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
                QueueDecision::Hold => {
                    let from = match &agents[agent_idx].task_leg {
                        TaskLeg::TravelToQueue { from, .. }
                        | TaskLeg::Queuing { from, .. } => *from,
                        _ => agents[agent_idx].pos,
                    };
                    let agent = &mut agents[agent_idx];
                    agent.task_leg = TaskLeg::Loading(from);
                    // Goal = pickup cell (from), NOT agent.pos.
                    // Setting goal = pos at a random corridor cell would trigger
                    // premature state transitions in recycle_goals_core (pos == goal).
                    agent.goal = from;
                    agent.planned_path.clear();
                    changed.push(agent_idx);
                }
            }
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
        let states = vec![
            QueueState::new(0, lines[0].capacity()),
            QueueState::new(1, lines[1].capacity()),
        ];

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
        let mut states = vec![
            QueueState::new(0, lines[0].capacity()),
            QueueState::new(1, lines[1].capacity()),
        ];
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
        let lines = vec![
            QueueLine::compute(IVec2::new(8, 2), Direction::West, &grid).unwrap(),
        ];
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
        let mut states = vec![
            QueueState::new(0, lines[0].capacity()),
            QueueState::new(1, lines[1].capacity()),
        ];
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
        let directions = vec![
            (IVec2::new(8, 2), Direction::West),
            (IVec2::new(6, 2), Direction::West),
        ];
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
}
