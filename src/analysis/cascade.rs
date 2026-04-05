use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::constants;
use crate::core::state::SimulationConfig;
use crate::fault::breakdown::FaultEvent;
use crate::fault::config::{FaultSource, FaultType};

use super::dependency::{ActionDependencyGraph, IndexedDependencyGraph};

/// Per-agent cascade record — counting only, no artificial delay numbers.
#[derive(Component, Debug, Clone)]
pub struct DelayRecord {
    /// Root fault entity that started this cascade.
    pub fault_origin: Entity,
    /// BFS depth from the faulted agent (0 = faulted agent itself).
    pub depth: u32,
}

/// Summary of a single fault event's cascade impact.
#[derive(Debug, Clone)]
pub struct CascadeFaultEntry {
    pub tick: u64,
    pub faulted_entity: Entity,
    pub fault_type: FaultType,
    pub source: FaultSource,
    pub position: IVec2,
    /// Number of agents affected by cascade (excluding the faulted agent).
    pub agents_affected: u32,
    /// Maximum BFS depth reached.
    pub max_depth: u32,
}

/// Accumulated cascade state across the entire simulation run.
#[derive(Resource, Debug, Default)]
pub struct CascadeState {
    pub records: HashMap<Entity, DelayRecord>,
    pub max_depth: u32,
    pub fault_count: u32,
    pub fault_log: Vec<CascadeFaultEntry>,
}

impl CascadeState {
    pub fn clear(&mut self) {
        self.records.clear();
        self.max_depth = 0;
        self.fault_count = 0;
        self.fault_log.clear();
    }

    /// Truncate data after `tick` for rewind support.
    pub fn truncate_after_tick(&mut self, tick: u64) {
        self.fault_log.retain(|e| e.tick <= tick);
        self.fault_count = self.fault_log.len() as u32;
        self.records.clear();
        self.max_depth = self.fault_log.iter().map(|e| e.max_depth).max().unwrap_or(0);
    }
}

/// On each tick, read FaultEvents. For each fault, BFS through the ADG
/// and count affected agents + depth. No artificial delay numbers.
/// Cascade depth is capped at `constants::MAX_CASCADE_DEPTH` to bound cost.
pub fn propagate_cascade(
    mut commands: Commands,
    mut fault_events: MessageReader<FaultEvent>,
    adg: Res<ActionDependencyGraph>,
    mut cascade: ResMut<CascadeState>,
    sim_config: Res<SimulationConfig>,
) {
    for event in fault_events.read() {
        let fault_origin = event.entity;
        let mut visited = HashSet::new();
        // BFS queue: (entity, depth) — no chain cloning needed
        let mut queue: VecDeque<(Entity, u32)> = VecDeque::new();

        // Start BFS from the faulted agent
        visited.insert(fault_origin);
        queue.push_back((fault_origin, 0));

        let mut event_affected = 0u32;
        let mut event_max_depth = 0u32;

        while let Some((entity, depth)) = queue.pop_front() {
            if depth > 0 {
                event_affected += 1;
                event_max_depth = event_max_depth.max(depth);

                // Insert or update DelayRecord (depth tracking only)
                let record =
                    cascade.records.entry(entity).or_insert(DelayRecord { fault_origin, depth: 0 });
                record.fault_origin = fault_origin;
                record.depth = record.depth.max(depth);

                commands.entity(entity).insert(DelayRecord { fault_origin, depth: record.depth });
            }

            // Cap BFS depth to avoid runaway cascades at large agent counts
            if depth >= constants::MAX_CASCADE_DEPTH {
                continue;
            }

            // Expand BFS to dependents
            for &dependent in adg.direct_dependents(entity) {
                if visited.insert(dependent) {
                    queue.push_back((dependent, depth + 1));
                }
            }
        }

        // Use the larger of ADG-based cascade and pre-replan path invalidation count.
        // ADG BFS misses obstacle-creation impact because it runs after replanning;
        // paths_invalidated captures agents whose paths crossed the dead cell at
        // the instant of death.
        let total_affected = event_affected.max(event.paths_invalidated);

        cascade.max_depth = cascade.max_depth.max(event_max_depth);
        cascade.fault_count += 1;

        cascade.fault_log.push(CascadeFaultEntry {
            tick: sim_config.tick,
            faulted_entity: event.entity,
            fault_type: event.fault_type,
            source: event.source,
            position: event.position,
            agents_affected: total_affected,
            max_depth: event_max_depth,
        });
    }
}

// ---------------------------------------------------------------------------
// Standalone cascade BFS — for experiment runner
// ---------------------------------------------------------------------------

/// Standalone cascade BFS for headless use.
/// Returns (agents_affected, max_depth) for a given dead agent index.
/// Mirrors the BFS logic of `propagate_cascade` but without ECS dependencies.
pub fn cascade_bfs_standalone(
    graph: &IndexedDependencyGraph,
    dead_agent: usize,
    max_depth: u32,
) -> (u32, u32) {
    let mut visited = HashSet::new();
    let mut queue: VecDeque<(usize, u32)> = VecDeque::new();
    visited.insert(dead_agent);
    queue.push_back((dead_agent, 0));

    let mut affected = 0u32;
    let mut deepest = 0u32;

    while let Some((idx, depth)) = queue.pop_front() {
        if depth > 0 {
            affected += 1;
            deepest = deepest.max(depth);
        }
        if depth >= max_depth {
            continue;
        }
        for &dep in graph.direct_dependents(idx) {
            if visited.insert(dep) {
                queue.push_back((dep, depth + 1));
            }
        }
    }

    (affected, deepest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test entity using the non-zero u32 constructor.
    fn entity(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("non-zero entity index must be valid")
    }

    // ── DelayRecord ──────────────────────────────────────────────────────

    #[test]
    fn delay_record_stores_depth() {
        let e = entity(1);
        let record = DelayRecord { fault_origin: e, depth: 3 };
        assert_eq!(record.depth, 3);
        assert_eq!(record.fault_origin, e);
    }

    // ── CascadeState::clear ───────────────────────────────────────────────

    #[test]
    fn cascade_state_clear_resets_all_fields() {
        let e = entity(1);
        let mut state = CascadeState {
            records: {
                let mut m = HashMap::new();
                m.insert(e, DelayRecord { fault_origin: e, depth: 1 });
                m
            },
            max_depth: 5,
            fault_count: 3,
            fault_log: vec![CascadeFaultEntry {
                tick: 10,
                faulted_entity: e,
                fault_type: FaultType::Breakdown,
                source: FaultSource::Automatic,
                position: IVec2::ZERO,
                agents_affected: 4,
                max_depth: 2,
            }],
        };

        state.clear();

        assert!(state.records.is_empty());
        assert_eq!(state.max_depth, 0);
        assert_eq!(state.fault_count, 0);
        assert!(state.fault_log.is_empty());
    }

    #[test]
    fn cascade_state_default_is_empty() {
        let state = CascadeState::default();
        assert!(state.records.is_empty());
        assert_eq!(state.max_depth, 0);
        assert_eq!(state.fault_count, 0);
        assert!(state.fault_log.is_empty());
    }

    // ── CascadeFaultEntry ─────────────────────────────────────────────────

    #[test]
    fn cascade_fault_entry_stores_fields() {
        let e = entity(5);
        let entry = CascadeFaultEntry {
            tick: 100,
            faulted_entity: e,
            fault_type: FaultType::Overheat,
            source: FaultSource::Automatic,
            position: IVec2::new(3, 4),
            agents_affected: 7,
            max_depth: 4,
        };
        assert_eq!(entry.tick, 100);
        assert_eq!(entry.faulted_entity, e);
        assert_eq!(entry.fault_type, FaultType::Overheat);
        assert_eq!(entry.source, FaultSource::Automatic);
        assert_eq!(entry.position, IVec2::new(3, 4));
        assert_eq!(entry.agents_affected, 7);
        assert_eq!(entry.max_depth, 4);
    }

    #[test]
    fn multiple_clear_calls_are_idempotent() {
        let mut state = CascadeState::default();
        state.max_depth = 10;
        state.clear();
        state.clear(); // second clear on already-empty state is safe
        assert_eq!(state.max_depth, 0);
    }

    // ── Standalone cascade BFS ───────────────────────────────────────

    #[test]
    fn test_cascade_bfs_no_dependents() {
        use super::super::dependency::IndexedDependencyGraph;
        let graph = IndexedDependencyGraph { dependents: HashMap::new() };
        let (affected, depth) = cascade_bfs_standalone(&graph, 0, 10);
        assert_eq!(affected, 0);
        assert_eq!(depth, 0);
    }

    #[test]
    fn test_cascade_bfs_linear_chain() {
        use super::super::dependency::IndexedDependencyGraph;
        // Chain: 0 → 1 → 2
        let mut deps = HashMap::new();
        deps.insert(0, vec![1]);
        deps.insert(1, vec![2]);
        let graph = IndexedDependencyGraph { dependents: deps };
        let (affected, depth) = cascade_bfs_standalone(&graph, 0, 10);
        assert_eq!(affected, 2);
        assert_eq!(depth, 2);
    }

    #[test]
    fn test_cascade_bfs_depth_cap() {
        use super::super::dependency::IndexedDependencyGraph;
        // Chain: 0 → 1 → 2 → 3, but max_depth = 1
        let mut deps = HashMap::new();
        deps.insert(0, vec![1]);
        deps.insert(1, vec![2]);
        deps.insert(2, vec![3]);
        let graph = IndexedDependencyGraph { dependents: deps };
        let (affected, depth) = cascade_bfs_standalone(&graph, 0, 1);
        assert_eq!(affected, 1); // only agent 1 reached
        assert_eq!(depth, 1);
    }
}
