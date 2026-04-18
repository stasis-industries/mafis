use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::constants;
use crate::core::grid::GridMap;
use crate::core::runner::SimAgent;
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
    /// Total agents impacted (including the faulted agent).
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
        let total_affected = event_affected.max(event.paths_invalidated) + 1;

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

    (affected + 1, deepest)
}

/// Like `cascade_bfs_standalone` but also marks every visited agent
/// (including the faulted agent) in `affected`. Used by the headless
/// experiment runner to populate the `ever_affected` trace required for
/// the Attack Rate metric (Wallinga & Lipsitch 2007).
///
/// Out-of-range indices in `affected` are silently skipped — callers are
/// responsible for sizing the slice to the agent count.
pub fn cascade_bfs_mark(
    graph: &IndexedDependencyGraph,
    dead_agent: usize,
    max_depth: u32,
    affected: &mut [bool],
) {
    let mut visited = HashSet::new();
    let mut queue: VecDeque<(usize, u32)> = VecDeque::new();
    visited.insert(dead_agent);
    queue.push_back((dead_agent, 0));

    while let Some((idx, depth)) = queue.pop_front() {
        if let Some(slot) = affected.get_mut(idx) {
            *slot = true;
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
}

// ---------------------------------------------------------------------------
// Structural cascade — solver-independent topological vulnerability
// ---------------------------------------------------------------------------

/// Result of a structural cascade evaluation at a single fault event.
#[derive(Debug, Clone, Copy, Default)]
pub struct StructuralCascade {
    /// Count of currently-alive agents whose shortest path from current
    /// position to current goal passes through the dead cell. Solver-independent.
    pub agents_disrupted: u32,
    /// Maximum BFS distance, over disrupted agents, from the dead cell to the
    /// agent's current position. Bounds the spatial reach of the structural
    /// vulnerability.
    pub max_distance: u32,
}

/// Structural cascade — solver-independent topological vulnerability of a
/// dead cell to fault impact.
///
/// Counts the number of currently-alive agents whose shortest path from their
/// current position to their current goal passes through `dead_cell`. The grid
/// is treated as static (no replanning, no solver involvement); only topology
/// and instantaneous agent state at the fault event matter.
///
/// **Conceptual lineage.** Weighted betweenness centrality (Freeman 1977;
/// Brandes 2001) with weights derived from actual lifelong agent state at
/// the fault event, rather than from uniform-random start/goal pairs as in
/// Ewing et al. (AAMAS 2022, betweenness for MAPF instance-hardness).
/// Adapted from cascading-failure analysis in transport/power networks
/// (Motter & Lai 2002; Jenelius 2009) to MAPF fault-cascade measurement.
///
/// **Why it matters.** The ADG-based cascade metric (`cascade_bfs_standalone`)
/// is biased by per-solver planning style: solvers that plan farther ahead
/// produce deeper ADG dependency chains, which inflates cascade depth even
/// when the topology is identical. Structural cascade measures the intrinsic
/// vulnerability of the cell, decoupled from planning horizon. The difference
/// `solver_cascade - structural_cascade` quantifies the solver's localization
/// (mitigation) skill.
///
/// **Algorithm.** One BFS from `dead_cell` over the static grid yields
/// `d_X(c)` for every reachable cell. For each alive agent A with pos != goal,
/// run a single-pair BFS to get `d(A.pos, A.goal)` in the original grid. The
/// dead cell is on at least one shortest path of A iff
/// `d_X(A.pos) + d_X(A.goal) == d(A.pos, A.goal)`.
///
/// Cost: O(V + N·V) per fault event, where V is grid cells and N is alive
/// agents. Dead and idle (pos == goal) agents are skipped.
pub fn structural_cascade_at(
    grid: &GridMap,
    dead_cell: IVec2,
    agents: &[SimAgent],
) -> StructuralCascade {
    let d_x = bfs_distances_from(grid, dead_cell);

    let mut disrupted = 0u32;
    let mut max_distance = 0u32;

    for agent in agents {
        if !agent.alive || agent.pos == agent.goal {
            continue;
        }
        let d_pos = match d_x.get(&agent.pos) {
            Some(&d) => d,
            None => continue,
        };
        let d_goal = match d_x.get(&agent.goal) {
            Some(&d) => d,
            None => continue,
        };
        let sum_via_x = match d_pos.checked_add(d_goal) {
            Some(s) => s,
            None => continue,
        };
        let d_direct = match bfs_shortest_distance(grid, agent.pos, agent.goal) {
            Some(d) => d,
            None => continue,
        };
        if sum_via_x == d_direct {
            disrupted += 1;
            max_distance = max_distance.max(d_pos);
        }
    }

    StructuralCascade { agents_disrupted: disrupted, max_distance }
}

/// BFS over the static grid from `source`. Returns the distance map from
/// source to every reachable walkable cell. The source is included with
/// distance 0 if walkable, otherwise still seeded so that adjacent walkable
/// cells get distance 1 (this matters when the dead cell itself is now an
/// obstacle but we still want to measure routes adjacent to it).
fn bfs_distances_from(grid: &GridMap, source: IVec2) -> HashMap<IVec2, u32> {
    let mut dist: HashMap<IVec2, u32> = HashMap::new();
    let mut queue: VecDeque<(IVec2, u32)> = VecDeque::new();
    dist.insert(source, 0);
    queue.push_back((source, 0));

    while let Some((pos, d)) = queue.pop_front() {
        for next in grid.walkable_neighbors(pos) {
            if dist.contains_key(&next) {
                continue;
            }
            dist.insert(next, d + 1);
            queue.push_back((next, d + 1));
        }
    }
    dist
}

/// Single-pair BFS shortest distance from `source` to `target` over the
/// static grid. Returns `None` if `target` is unreachable.
fn bfs_shortest_distance(grid: &GridMap, source: IVec2, target: IVec2) -> Option<u32> {
    if source == target {
        return Some(0);
    }
    let mut visited: HashSet<IVec2> = HashSet::new();
    let mut queue: VecDeque<(IVec2, u32)> = VecDeque::new();
    visited.insert(source);
    queue.push_back((source, 0));

    while let Some((pos, d)) = queue.pop_front() {
        for next in grid.walkable_neighbors(pos) {
            if next == target {
                return Some(d + 1);
            }
            if !visited.insert(next) {
                continue;
            }
            queue.push_back((next, d + 1));
        }
    }
    None
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
        let mut state = CascadeState { max_depth: 10, ..Default::default() };
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
        assert_eq!(affected, 1); // the faulted agent itself
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
        assert_eq!(affected, 3); // faulted + 2 cascade
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
        assert_eq!(affected, 2); // faulted + agent 1
        assert_eq!(depth, 1);
    }

    // ── cascade_bfs_mark (Attack Rate trace) ─────────────────────────

    #[test]
    fn cascade_bfs_mark_marks_linear_chain() {
        use super::super::dependency::IndexedDependencyGraph;
        // Chain: 0 → 1 → 2
        let mut deps = HashMap::new();
        deps.insert(0, vec![1]);
        deps.insert(1, vec![2]);
        let graph = IndexedDependencyGraph { dependents: deps };
        let mut affected = vec![false; 3];
        cascade_bfs_mark(&graph, 0, 10, &mut affected);
        assert_eq!(affected, vec![true, true, true]);
    }

    #[test]
    fn cascade_bfs_mark_no_dependents_marks_only_faulted() {
        use super::super::dependency::IndexedDependencyGraph;
        let graph = IndexedDependencyGraph { dependents: HashMap::new() };
        let mut affected = vec![false; 3];
        cascade_bfs_mark(&graph, 1, 10, &mut affected);
        assert_eq!(affected, vec![false, true, false]);
    }

    #[test]
    fn cascade_bfs_mark_respects_depth_cap() {
        use super::super::dependency::IndexedDependencyGraph;
        // Chain: 0 → 1 → 2 → 3, depth cap = 1
        let mut deps = HashMap::new();
        deps.insert(0, vec![1]);
        deps.insert(1, vec![2]);
        deps.insert(2, vec![3]);
        let graph = IndexedDependencyGraph { dependents: deps };
        let mut affected = vec![false; 4];
        cascade_bfs_mark(&graph, 0, 1, &mut affected);
        // depth 0 = agent 0, depth 1 = agent 1. Agents 2, 3 NOT marked.
        assert_eq!(affected, vec![true, true, false, false]);
    }

    #[test]
    fn cascade_bfs_mark_accumulates_across_calls() {
        use super::super::dependency::IndexedDependencyGraph;
        // Two disjoint chains: 0→1, 2→3
        let mut deps = HashMap::new();
        deps.insert(0, vec![1]);
        deps.insert(2, vec![3]);
        let graph = IndexedDependencyGraph { dependents: deps };
        let mut affected = vec![false; 4];
        cascade_bfs_mark(&graph, 0, 10, &mut affected);
        cascade_bfs_mark(&graph, 2, 10, &mut affected);
        assert_eq!(affected, vec![true, true, true, true]);
    }

    #[test]
    fn cascade_bfs_mark_out_of_range_safe() {
        use super::super::dependency::IndexedDependencyGraph;
        let graph = IndexedDependencyGraph { dependents: HashMap::new() };
        let mut affected = vec![false; 2];
        // idx=5 > slice len — must not panic
        cascade_bfs_mark(&graph, 5, 10, &mut affected);
        assert_eq!(affected, vec![false, false]);
    }

    // ── Structural cascade (solver-independent) ───────────────────────

    fn agent_at(pos: IVec2, goal: IVec2) -> SimAgent {
        let mut a = SimAgent::new(pos);
        a.goal = goal;
        a.alive = true;
        a
    }

    #[test]
    fn structural_cascade_open_corridor_one_agent() {
        // 5×1 corridor: agent at (0,0) heading to (4,0). Dead cell at (2,0).
        // The unique shortest path goes through (2,0), so the agent must be
        // counted as disrupted.
        let grid = GridMap::new(5, 1);
        let agents = vec![agent_at(IVec2::new(0, 0), IVec2::new(4, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &agents);
        assert_eq!(r.agents_disrupted, 1, "agent on unique corridor must be flagged");
        assert_eq!(r.max_distance, 2, "dead cell is 2 steps from agent.pos");
    }

    #[test]
    fn structural_cascade_open_grid_offcorridor_alternative_paths() {
        // 5×5 fully open grid: agent at (0,0) → (4,0). Dead cell at (2,2)
        // is NOT on any (0,0) → (4,0) shortest path (Manhattan = 4, going
        // through (2,2) costs 4+4 = 8). Should NOT be flagged.
        let grid = GridMap::new(5, 5);
        let agents = vec![agent_at(IVec2::new(0, 0), IVec2::new(4, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(2, 2), &agents);
        assert_eq!(r.agents_disrupted, 0, "off-corridor cell must not be flagged");
    }

    #[test]
    fn structural_cascade_open_grid_oncorridor_alternative_paths() {
        // 5×5 fully open grid: agent at (0,0) → (4,0). Cell (2,0) IS on a
        // shortest path (Manhattan = 4 via (2,0)). Counts as disrupted even
        // though alternative shortest paths exist (the metric is "X on at
        // least one shortest path", the betweenness analog).
        let grid = GridMap::new(5, 5);
        let agents = vec![agent_at(IVec2::new(0, 0), IVec2::new(4, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &agents);
        assert_eq!(r.agents_disrupted, 1, "on-corridor cell must be flagged");
        assert_eq!(r.max_distance, 2);
    }

    #[test]
    fn structural_cascade_skips_dead_agents() {
        let grid = GridMap::new(5, 1);
        let mut a = agent_at(IVec2::new(0, 0), IVec2::new(4, 0));
        a.alive = false;
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &[a]);
        assert_eq!(r.agents_disrupted, 0, "dead agents must be excluded");
    }

    #[test]
    fn structural_cascade_skips_idle_agents() {
        // Agent at goal already (pos == goal): no path required, nothing to
        // disrupt.
        let grid = GridMap::new(5, 1);
        let agents = vec![agent_at(IVec2::new(2, 0), IVec2::new(2, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &agents);
        assert_eq!(r.agents_disrupted, 0, "idle agents must be excluded");
    }

    #[test]
    fn structural_cascade_aggregates_multiple_agents() {
        // 5×1 corridor: 3 agents all needing to traverse cell (2,0).
        let grid = GridMap::new(5, 1);
        let agents = vec![
            agent_at(IVec2::new(0, 0), IVec2::new(4, 0)),
            agent_at(IVec2::new(1, 0), IVec2::new(3, 0)),
            agent_at(IVec2::new(0, 0), IVec2::new(3, 0)),
        ];
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &agents);
        assert_eq!(r.agents_disrupted, 3);
        // Max distance from dead cell (2,0) to any agent.pos is 2 (agent 0).
        assert_eq!(r.max_distance, 2);
    }

    #[test]
    fn structural_cascade_unreachable_returns_zero() {
        // Two disconnected components separated by a wall column.
        // Agent in the left component has goal in the same left component;
        // dead cell in the right component is unreachable from the agent's
        // position, so it cannot be on any shortest path.
        let mut grid = GridMap::new(5, 1);
        // Wall at x=2, splitting (0,1) from (3,4).
        grid.set_obstacle(IVec2::new(2, 0));
        let agents = vec![agent_at(IVec2::new(0, 0), IVec2::new(1, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(4, 0), &agents);
        assert_eq!(r.agents_disrupted, 0, "unreachable dead cell must not be flagged");
    }

    #[test]
    fn structural_cascade_solver_independent() {
        // Determinism check: structural cascade depends ONLY on (grid,
        // dead_cell, agent.pos, agent.goal). Two SimAgents with different
        // planned_path/heat/operational_age but same pos/goal must yield
        // identical results.
        use crate::core::action::{Action, Direction};
        use std::collections::VecDeque;
        let grid = GridMap::new(5, 1);
        let mut a1 = agent_at(IVec2::new(0, 0), IVec2::new(4, 0));
        let mut a2 = agent_at(IVec2::new(0, 0), IVec2::new(4, 0));
        a1.planned_path = VecDeque::from(vec![Action::Move(Direction::East); 4]);
        a2.planned_path = VecDeque::from(vec![Action::Wait]);
        a1.heat = 5.0;
        a2.heat = 0.0;
        a1.operational_age = 100;
        a2.operational_age = 0;
        let r1 = structural_cascade_at(&grid, IVec2::new(2, 0), &[a1]);
        let r2 = structural_cascade_at(&grid, IVec2::new(2, 0), &[a2]);
        assert_eq!(r1.agents_disrupted, r2.agents_disrupted);
        assert_eq!(r1.max_distance, r2.max_distance);
    }

    #[test]
    fn bfs_shortest_distance_self_is_zero() {
        let grid = GridMap::new(5, 5);
        assert_eq!(bfs_shortest_distance(&grid, IVec2::new(2, 2), IVec2::new(2, 2)), Some(0));
    }

    #[test]
    fn bfs_shortest_distance_open_grid_manhattan() {
        let grid = GridMap::new(5, 5);
        // Open 5x5: shortest from (0,0) to (3,2) is Manhattan = 5
        assert_eq!(bfs_shortest_distance(&grid, IVec2::new(0, 0), IVec2::new(3, 2)), Some(5));
    }

    #[test]
    fn bfs_shortest_distance_unreachable_returns_none() {
        let mut grid = GridMap::new(5, 1);
        // Block off (2,0); now (0,0) cannot reach (4,0).
        grid.set_obstacle(IVec2::new(2, 0));
        assert_eq!(bfs_shortest_distance(&grid, IVec2::new(0, 0), IVec2::new(4, 0)), None);
    }

    #[test]
    fn bfs_distances_from_seeds_source_at_zero() {
        let grid = GridMap::new(3, 1);
        let d = bfs_distances_from(&grid, IVec2::new(0, 0));
        assert_eq!(d.get(&IVec2::new(0, 0)), Some(&0));
        assert_eq!(d.get(&IVec2::new(1, 0)), Some(&1));
        assert_eq!(d.get(&IVec2::new(2, 0)), Some(&2));
    }
}
