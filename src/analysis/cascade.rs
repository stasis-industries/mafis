use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::constants;
use crate::core::grid::GridMap;
use crate::core::runner::{FaultSnapshot, SimAgent};
use crate::core::state::SimulationConfig;
use crate::fault::breakdown::FaultEvent;
use crate::fault::config::{FaultSource, FaultType};

use super::dependency::{ActionDependencyGraph, IndexedDependencyGraph, build_adg_from_snapshot};

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

/// On each tick, read FaultEvents and record each fault's cascade. Runner
/// faults carry the cascade computed at the fault instant (same definition as
/// the experiment runner); other faults BFS through the live ADG.
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

        if let Some((event_max_depth, reached)) = &event.cascade {
            for &(entity, depth) in reached {
                let record =
                    cascade.records.entry(entity).or_insert(DelayRecord { fault_origin, depth: 0 });
                record.fault_origin = fault_origin;
                record.depth = record.depth.max(depth);
                commands.entity(entity).insert(DelayRecord { fault_origin, depth: record.depth });
            }
            cascade.max_depth = cascade.max_depth.max(*event_max_depth);
            cascade.fault_count += 1;
            cascade.fault_log.push(CascadeFaultEntry {
                tick: sim_config.tick,
                faulted_entity: event.entity,
                fault_type: event.fault_type,
                source: event.source,
                position: event.position,
                agents_affected: reached.len() as u32 + 1,
                max_depth: *event_max_depth,
            });
            continue;
        }

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

/// Cascade of one fault group: agents reached through the ADG (the faulted
/// agents themselves excluded) and the deepest BFS level reached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CascadeOutcome {
    /// (agent, BFS depth from the nearest faulted agent).
    pub reached: Vec<(usize, u32)>,
    pub max_depth: u32,
}

/// Multi-source BFS from every agent faulted at the same instant. A reached
/// agent's depth is its distance to the nearest faulted agent.
pub fn cascade_bfs(
    graph: &IndexedDependencyGraph,
    sources: &[usize],
    max_depth: u32,
) -> CascadeOutcome {
    let mut visited: HashSet<usize> = sources.iter().copied().collect();
    let mut queue: VecDeque<(usize, u32)> = sources.iter().map(|&s| (s, 0)).collect();
    let mut out = CascadeOutcome::default();

    while let Some((idx, depth)) = queue.pop_front() {
        if depth > 0 {
            out.reached.push((idx, depth));
            out.max_depth = out.max_depth.max(depth);
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
    out
}

/// Cascade of a fault group from the state captured at the fault instant.
pub fn cascade_from_snapshot(snapshot: &FaultSnapshot, max_depth: u32) -> CascadeOutcome {
    cascade_bfs(&build_adg_from_snapshot(snapshot), &snapshot.sources, max_depth)
}

/// Secondary attack rate (epidemiology): among agents never directly faulted,
/// the fraction reached by any fault's cascade. Directly faulted agents are
/// primary cases and excluded from both sides, so the value does not echo the
/// configured kill fraction. NaN when every agent was directly faulted.
pub fn secondary_attack_rate(directly_hit: &[bool], reached: &[bool]) -> f64 {
    let (secondary, susceptible) = directly_hit
        .iter()
        .zip(reached)
        .filter(|&(&hit, _)| !hit)
        .fold((0usize, 0usize), |(s, n), (_, &r)| (s + usize::from(r), n + 1));
    if susceptible == 0 { f64::NAN } else { secondary as f64 / susceptible as f64 }
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
    // The betweenness identity `d_X(s) + d_X(t) == d_direct` only holds when
    // `dead_cell` is walkable in the graph used for all three BFS computations.
    // For permanent faults the runner already calls `grid.set_obstacle(dead_cell)`
    // before this function runs (see runner/mod.rs), so we work on a clone with
    // the obstacle cleared. For latency/intermittent/zone faults the cell stays
    // walkable, so we borrow without cloning.
    let working_grid: std::borrow::Cow<'_, GridMap> = if grid.is_obstacle(dead_cell) {
        let mut g = grid.clone();
        g.remove_obstacle(dead_cell);
        std::borrow::Cow::Owned(g)
    } else {
        std::borrow::Cow::Borrowed(grid)
    };
    let grid: &GridMap = &working_grid;

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
    let mut dist: HashMap<IVec2, u32> = HashMap::with_capacity((grid.width * grid.height) as usize);
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
    let mut visited: HashSet<IVec2> = HashSet::with_capacity((grid.width * grid.height) as usize);
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

    fn chain_graph(edges: &[(usize, usize)]) -> IndexedDependencyGraph {
        let mut deps: HashMap<usize, Vec<usize>> = HashMap::new();
        for &(a, b) in edges {
            deps.entry(a).or_default().push(b);
        }
        IndexedDependencyGraph { dependents: deps }
    }

    #[test]
    fn test_cascade_bfs_no_dependents() {
        let out = cascade_bfs(&chain_graph(&[]), &[0], 10);
        assert!(out.reached.is_empty());
        assert_eq!(out.max_depth, 0);
    }

    #[test]
    fn test_cascade_bfs_linear_chain() {
        let out = cascade_bfs(&chain_graph(&[(0, 1), (1, 2)]), &[0], 10);
        assert_eq!(out.reached, vec![(1, 1), (2, 2)]);
        assert_eq!(out.max_depth, 2);
    }

    #[test]
    fn test_cascade_bfs_depth_cap() {
        let out = cascade_bfs(&chain_graph(&[(0, 1), (1, 2), (2, 3)]), &[0], 1);
        assert_eq!(out.reached, vec![(1, 1)]);
        assert_eq!(out.max_depth, 1);
    }

    #[test]
    fn test_cascade_bfs_multi_source_uses_nearest_source() {
        // 0 → 1 → 2 → 3 with sources {0, 2}: agent 3 is depth 1 from source 2.
        let out = cascade_bfs(&chain_graph(&[(0, 1), (1, 2), (2, 3)]), &[0, 2], 10);
        assert_eq!(out.reached, vec![(1, 1), (3, 1)]);
        assert_eq!(out.max_depth, 1);
    }

    // ── Fault-instant snapshot cascade ───────────────────────────────

    /// Agents queued East-bound in a 1-wide corridor: index 0 is the head at
    /// x = `followers`, index k sits at x = `followers - k`. Every agent plans
    /// `plan_len` East moves (1 = PIBT-style, longer = RHCR/TP-style paths).
    fn corridor_queue(followers: i32, plan_len: usize) -> Vec<SimAgent> {
        use crate::core::action::{Action, Direction};
        (0..=followers)
            .map(|k| {
                let mut a = SimAgent::new(IVec2::new(followers - k, 0));
                a.planned_path = VecDeque::from(vec![Action::Move(Direction::East); plan_len]);
                a
            })
            .collect()
    }

    fn snapshot_cascade(agents: &[SimAgent], sources: &[usize]) -> CascadeOutcome {
        use crate::core::runner::FaultSnapshot;
        let mut snap = FaultSnapshot::capture(agents, constants::ADG_LOOKAHEAD);
        snap.sources = sources.to_vec();
        cascade_from_snapshot(&snap, constants::MAX_CASCADE_DEPTH)
    }

    #[test]
    fn snapshot_cascade_corridor_chain_depth_equals_chain_length() {
        // Head killed with 3 followers queued behind it: each follower's next
        // cell is the cell the agent ahead must vacate → chain of depth 3.
        let agents = corridor_queue(3, 1);
        let out = snapshot_cascade(&agents, &[0]);
        assert_eq!(out.max_depth, 3);
        let mut reached = out.reached.clone();
        reached.sort_unstable();
        assert_eq!(reached, vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn snapshot_cascade_same_for_any_solver_plan_length() {
        // Identical positions, plans of 1 (PIBT), 5 (RHCR window) and 30 (TP
        // full path) steps: the common lookahead makes depth solver-independent.
        let depths: Vec<u32> = [1, 5, 30]
            .iter()
            .map(|&len| snapshot_cascade(&corridor_queue(3, len), &[0]).max_depth)
            .collect();
        assert_eq!(depths, vec![3, 3, 3]);
    }

    #[test]
    fn snapshot_cascade_no_followers_is_zero() {
        // Followers plan to move away (West) — nobody needs the victim's cell.
        use crate::core::action::{Action, Direction};
        let mut agents = corridor_queue(2, 1);
        for a in agents.iter_mut().skip(1) {
            a.planned_path = VecDeque::from(vec![Action::Move(Direction::West)]);
        }
        let out = snapshot_cascade(&agents, &[0]);
        assert_eq!(out.max_depth, 0);
        assert!(out.reached.is_empty());
    }

    #[test]
    fn snapshot_cascade_multi_source_counts_victims_as_roots() {
        // Burst kills the head and the 2nd follower: agent 1 is reached from
        // agent 0 (depth 1); agent 3 is reached from agent 2 (depth 1).
        let agents = corridor_queue(3, 1);
        let out = snapshot_cascade(&agents, &[0, 2]);
        assert_eq!(out.max_depth, 1);
        let mut reached = out.reached.clone();
        reached.sort_unstable();
        assert_eq!(reached, vec![(1, 1), (3, 1)]);
    }

    #[test]
    fn snapshot_dead_agents_are_not_nodes() {
        // An already-dead agent between victim and follower breaks the chain.
        let mut agents = corridor_queue(2, 1);
        agents[1].alive = false;
        let out = snapshot_cascade(&agents, &[0]);
        assert_eq!(out.max_depth, 0);
    }

    #[test]
    fn secondary_attack_rate_excludes_primary_cases() {
        // 4 agents: 0 directly hit, 1 reached, 2 reached then directly hit
        // later (primary), 3 untouched → 1 secondary of 2 never-hit agents.
        let hit = [true, false, true, false];
        let reached = [false, true, true, false];
        assert!((secondary_attack_rate(&hit, &reached) - 0.5).abs() < 1e-12);
        assert_eq!(secondary_attack_rate(&[false; 3], &[false; 3]), 0.0);
        assert!(secondary_attack_rate(&[true; 2], &[false; 2]).is_nan());
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

    /// Regression for the post-fault grid bug: the runner mutates the grid
    /// (`set_obstacle(dead_cell)`) before this analysis runs for permanent
    /// faults. The betweenness identity must still hold by reconstructing a
    /// pre-fault view internally.
    #[test]
    fn structural_cascade_post_fault_grid_corridor_one_agent() {
        // Same scenario as above, but the dead cell is already an obstacle —
        // matching the production state for Breakdown / Wear / Burst faults.
        let mut grid = GridMap::new(5, 1);
        grid.set_obstacle(IVec2::new(2, 0));
        let agents = vec![agent_at(IVec2::new(0, 0), IVec2::new(4, 0))];
        let r = structural_cascade_at(&grid, IVec2::new(2, 0), &agents);
        assert_eq!(
            r.agents_disrupted, 1,
            "post-fault grid: agent on unique corridor must still be flagged"
        );
        assert_eq!(r.max_distance, 2);
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
