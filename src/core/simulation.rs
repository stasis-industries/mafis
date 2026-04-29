use bevy::prelude::*;
use std::collections::HashMap;

use super::action::Action;
use super::grid::GridMap;

// ---------------------------------------------------------------------------
// Core collision resolution (shared by ECS and headless baseline)
// ---------------------------------------------------------------------------
//
// `tick_agents_core` is the pure-function collision resolver shared between
// the ECS path and the headless baseline. There is no plugin: the live tick
// chain is registered by `CorePlugin` (see `core/mod.rs`).

/// Input for one agent to collision resolution.
pub struct AgentMoveInput {
    pub current_pos: IVec2,
    pub desired_action: Action,
}

/// Result for one agent from collision resolution.
pub struct ResolvedMove {
    pub new_pos: IVec2,
    pub action: Action,
    pub was_forced: bool,
}

/// Pure collision resolution: desired moves + dead positions + grid → resolved moves.
///
/// Both the live ECS `tick_agents` system and the headless baseline call this.
/// Agents must be passed in deterministic order (sorted by AgentIndex).
pub fn tick_agents_core(
    agents: &[AgentMoveInput],
    dead_positions: &[IVec2],
    grid: &GridMap,
) -> Vec<ResolvedMove> {
    let n = agents.len();
    if n == 0 {
        return Vec::new();
    }

    // Phase 1: Compute desired moves
    // (current_pos, action, target, was_forced)
    let mut moves: Vec<(IVec2, Action, IVec2, bool)> = Vec::with_capacity(n);
    for a in agents {
        let new_pos = a.desired_action.apply(a.current_pos);
        let target = if grid.is_walkable(new_pos) { new_pos } else { a.current_pos };
        moves.push((a.current_pos, a.desired_action, target, false));
    }

    // Build dead agent occupation map
    let mut occupied: HashMap<IVec2, usize> = HashMap::with_capacity(dead_positions.len());
    for (i, &pos) in dead_positions.iter().enumerate() {
        occupied.insert(pos, i);
    }

    // Phase 2: Iterative collision resolution
    let mut changed = true;
    let mut source_map: HashMap<IVec2, usize> = HashMap::with_capacity(n);
    while changed {
        changed = false;

        // Vertex conflicts: if two agents target the same cell, force one to wait.
        // Winner = staying agent (target == current), fallback = lowest index.
        let mut target_count: HashMap<IVec2, (usize, usize)> = HashMap::with_capacity(n);
        for (i, m) in moves.iter().enumerate() {
            target_count
                .entry(m.2)
                .and_modify(|(winner_idx, count)| {
                    *count += 1;
                    if m.2 == m.0 {
                        *winner_idx = i;
                    }
                })
                .or_insert((i, 1));
        }

        #[allow(clippy::needless_range_loop)]
        for i in 0..n {
            let target = moves[i].2;
            if let Some(&(winner_idx, count)) = target_count.get(&target) {
                if count <= 1 || i == winner_idx {
                    continue;
                }
                if moves[i].2 != moves[i].0 {
                    moves[i].1 = Action::Wait;
                    moves[i].2 = moves[i].0;
                    moves[i].3 = true;
                    changed = true;
                }
            }
        }

        // Edge swaps: agents A→B and B→A — O(n) via source-position map
        // Build map: source_pos → agent_index for all MOVING agents
        source_map.clear();
        for (i, m) in moves.iter().enumerate() {
            if m.2 != m.0 {
                source_map.insert(m.0, i);
            }
        }
        for i in 0..n {
            if moves[i].2 == moves[i].0 {
                continue;
            }
            // Check if there's an agent at our target that wants to move to our source
            if let Some(&j) = source_map.get(&moves[i].2)
                && j > i
                && moves[j].2 == moves[i].0
            {
                // Edge swap detected: force the higher-index agent to wait
                moves[j].1 = Action::Wait;
                moves[j].2 = moves[j].0;
                moves[j].3 = true;
                changed = true;
            }
        }

        // Dead agent collisions
        for m in moves.iter_mut() {
            if m.2 != m.0 && occupied.contains_key(&m.2) {
                m.1 = Action::Wait;
                m.2 = m.0;
                m.3 = true;
                changed = true;
            }
        }
    }

    moves.iter().map(|m| ResolvedMove { new_pos: m.2, action: m.1, was_forced: m.3 }).collect()
}
