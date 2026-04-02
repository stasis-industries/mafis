//! Queue assignment policies — swappable research variable.
//!
//! Each policy implements `DeliveryQueuePolicy` and decides which delivery
//! queue an agent should join based on current occupancy and position.

use bevy::prelude::IVec2;

use super::{QueueDecision, QueueLine, QueueState};

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
            let target = line.join_cell(state);
            let dist = (agent_pos.x - target.x).abs() + (agent_pos.y - target.y).abs();
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
            let target = line.join_cell(state);
            let dist = (agent_pos.x - target.x).abs() + (agent_pos.y - target.y).abs();
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
        Self { distance_weight: 0.5, occupancy_weight: 0.5 }
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
            let target = line.join_cell(state);
            let dist = ((agent_pos.x - target.x).abs() + (agent_pos.y - target.y).abs()) as f32;
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
            let target = line.join_cell(state);
            let dist = ((agent_pos.x - target.x).abs() + (agent_pos.y - target.y).abs()) as f32;
            let occ = state.occupancy() as f32;

            let score =
                self.distance_weight * (dist / max_dist) + self.occupancy_weight * (occ / max_occ);

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
