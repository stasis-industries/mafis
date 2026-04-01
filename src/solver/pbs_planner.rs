//! PBS (Priority-Based Search) planner for RHCR's windowed planning.
//!
//! Builds a binary priority tree: each node has a priority ordering. When a
//! conflict is found, the tree branches into two children (agent_i > agent_j
//! and agent_j > agent_i). Bounded by node limit.

use bevy::prelude::*;

use crate::core::action::Action;
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;

use super::astar::{Constraints, FlatConstraintIndex, SpacetimeGrid, SeqGoalGrid,
    spacetime_astar_guided, spacetime_astar_fast, spacetime_astar_sequential};
use super::heuristics::DistanceMap;
use super::windowed::{PlanFragment, WindowAgent, WindowContext, WindowResult, WindowedPlanner};

// ---------------------------------------------------------------------------
// PBS Node
// ---------------------------------------------------------------------------

/// A single node in the PBS priority tree.
struct PbsNode {
    /// Plans for each agent (index aligned with WindowContext.agents).
    plans: Vec<Vec<Action>>,
    /// Pre-built position timelines (avoids rebuilding for conflict detection).
    timelines: Vec<Vec<IVec2>>,
    /// Priority ordering constraints: (higher, lower) — `higher` plans first.
    priority_pairs: Vec<(usize, usize)>,
    /// Number of conflicts in this node (for best-first ordering).
    conflicts: usize,
    /// Earliest timestep at which a collision occurs (usize::MAX if none).
    earliest_collision: usize,
    /// Node ID for tie-breaking (lower = earlier).
    id: usize,
}


// ---------------------------------------------------------------------------
// Spatial conflict detection — O(n × H) replacing O(n² × H)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Conflict {
    agent_a: usize,
    agent_b: usize,
    #[allow(dead_code)]
    time: u64,
}

const NO_AGENT: u32 = u32::MAX;

/// Grid-indexed conflict detection: O(n × H) instead of O(n² × H).
struct ConflictGrid {
    /// Grid for current timestep: grid[pos_flat] = agent index
    cur: Vec<u32>,
    /// Grid for previous timestep positions
    prev: Vec<u32>,
    cells: usize,
}

impl ConflictGrid {
    fn new() -> Self {
        Self {
            cur: Vec::new(),
            prev: Vec::new(),
            cells: 0,
        }
    }

    fn ensure_size(&mut self, cells: usize) {
        if self.cells != cells {
            self.cells = cells;
            self.cur = vec![NO_AGENT; cells];
            self.prev = vec![NO_AGENT; cells];
        }
    }

    fn detect_first(&mut self, timelines: &[Vec<IVec2>], grid_w: i32, window: usize) -> Option<Conflict> {
        let n = timelines.len();
        if n < 2 {
            return None;
        }

        let max_t = timelines.iter().map(|tl| tl.len().saturating_sub(1)).max().unwrap_or(0);
        let max_t = max_t.min(window);

        // Fill t=0 positions into prev
        self.prev.fill(NO_AGENT);
        for (i, tl) in timelines.iter().enumerate() {
            let pos = pos_at(tl, 0);
            let flat = (pos.y * grid_w + pos.x) as usize;
            if flat < self.cells {
                self.prev[flat] = i as u32;
            }
        }

        for t in 1..=max_t {
            self.cur.fill(NO_AGENT);

            for i in 0..n {
                let pos = pos_at(&timelines[i], t);
                let flat = (pos.y * grid_w + pos.x) as usize;
                if flat >= self.cells {
                    continue;
                }

                // Vertex conflict: another agent already at this cell at time t
                if self.cur[flat] != NO_AGENT {
                    let j = self.cur[flat] as usize;
                    return Some(Conflict { agent_a: j, agent_b: i, time: t as u64 });
                }
                self.cur[flat] = i as u32;

                // Edge conflict: check if agent i swapped with someone
                let prev_pos = pos_at(&timelines[i], t - 1);
                if prev_pos != pos {
                    // Who was at my current position (pos) at t-1?
                    let pos_flat = (pos.y * grid_w + pos.x) as usize;
                    if pos_flat < self.cells {
                        let j = self.prev[pos_flat];
                        if j != NO_AGENT && j != i as u32 {
                            let j = j as usize;
                            // Agent j was at pos at t-1. If j is now at prev_pos, it's a swap.
                            let j_cur = pos_at(&timelines[j], t);
                            if j_cur == prev_pos {
                                return Some(Conflict { agent_a: i, agent_b: j, time: t as u64 });
                            }
                        }
                    }
                }
            }

            // Swap cur → prev for next iteration
            std::mem::swap(&mut self.cur, &mut self.prev);
        }

        None
    }

    fn count_conflicts(&mut self, timelines: &[Vec<IVec2>], grid_w: i32, window: usize) -> usize {
        let n = timelines.len();
        if n < 2 {
            return 0;
        }

        let max_t = timelines.iter().map(|tl| tl.len().saturating_sub(1)).max().unwrap_or(0);
        let max_t = max_t.min(window);
        let mut count = 0;

        // Fill t=0 positions into prev
        self.prev.fill(NO_AGENT);
        for (i, tl) in timelines.iter().enumerate() {
            let pos = pos_at(tl, 0);
            let flat = (pos.y * grid_w + pos.x) as usize;
            if flat < self.cells {
                self.prev[flat] = i as u32;
            }
        }

        for t in 1..=max_t {
            self.cur.fill(NO_AGENT);

            for i in 0..n {
                let pos = pos_at(&timelines[i], t);
                let flat = (pos.y * grid_w + pos.x) as usize;
                if flat >= self.cells {
                    continue;
                }

                // Vertex conflict
                if self.cur[flat] != NO_AGENT {
                    count += 1;
                }
                self.cur[flat] = i as u32;

                // Edge conflict (swap)
                let prev_pos = pos_at(&timelines[i], t - 1);
                if prev_pos != pos {
                    let pos_flat2 = (pos.y * grid_w + pos.x) as usize;
                    if pos_flat2 < self.cells {
                        let j = self.prev[pos_flat2];
                        if j != NO_AGENT && j != i as u32 {
                            let j_cur = pos_at(&timelines[j as usize], t);
                            if j_cur == prev_pos {
                                count += 1;
                            }
                        }
                    }
                }
            }

            std::mem::swap(&mut self.cur, &mut self.prev);
        }

        count
    }
}

/// Build position timelines from plans + starting positions.
fn build_timelines(plans: &[Vec<Action>], agents: &[WindowAgent]) -> Vec<Vec<IVec2>> {
    plans
        .iter()
        .zip(agents.iter())
        .map(|(plan, agent)| {
            let mut pos = agent.pos;
            let mut tl = Vec::with_capacity(plan.len() + 1);
            tl.push(pos);
            for &a in plan {
                pos = a.apply(pos);
                tl.push(pos);
            }
            tl
        })
        .collect()
}

/// Rebuild a single agent's timeline in-place.
fn rebuild_timeline(timelines: &mut [Vec<IVec2>], plans: &[Vec<Action>], agents: &[WindowAgent], idx: usize) {
    let plan = &plans[idx];
    let tl = &mut timelines[idx];
    tl.clear();
    let mut pos = agents[idx].pos;
    tl.push(pos);
    for &a in plan {
        pos = a.apply(pos);
        tl.push(pos);
    }
}

#[inline]
fn pos_at(timeline: &[IVec2], t: usize) -> IVec2 {
    if t < timeline.len() {
        timeline[t]
    } else {
        *timeline.last().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Plan one agent with priority constraints (using fast A*)
// ---------------------------------------------------------------------------

fn plan_agent(
    agent_idx: usize,
    agents: &[WindowAgent],
    all_plans: &[Vec<Action>],
    priority_pairs: &[(usize, usize)],
    grid: &GridMap,
    horizon: usize,
    dist_map: Option<&DistanceMap>,
    ci_buf: &mut FlatConstraintIndex,
    stg: &mut SpacetimeGrid,
) -> Option<Vec<Action>> {
    let agent = &agents[agent_idx];

    // Build flat constraint index for this agent
    ci_buf.reset(grid.width, grid.height, horizon as u64);

    for &(higher, lower) in priority_pairs {
        if lower == agent_idx {
            let plan = &all_plans[higher];
            let higher_agent = &agents[higher];
            let mut pos = higher_agent.pos;
            for (t, &action) in plan.iter().enumerate() {
                let next_pos = action.apply(pos);
                ci_buf.add_vertex(next_pos, (t + 1) as u64);
                ci_buf.add_edge(next_pos, pos, t as u64);
                pos = next_pos;
            }
            // After plan ends, agent stays at final position
            let final_t = plan.len();
            for t in final_t..(horizon + 1) {
                ci_buf.add_vertex(pos, t as u64);
            }
        }
    }

    spacetime_astar_fast(
        grid,
        agent.pos,
        agent.goal,
        ci_buf,
        horizon as u64,
        dist_map,
        stg,
        u64::MAX, // PBS has its own node limit via PBS_MAX_NODE_LIMIT
    )
    .ok()
}

// ---------------------------------------------------------------------------
// PbsPlanner
// ---------------------------------------------------------------------------

pub struct PbsPlanner {
    conflict_grid: ConflictGrid,
    ci_buf: FlatConstraintIndex,
    stg: SpacetimeGrid,
    seq_stg: SeqGoalGrid,
    empty_ci: FlatConstraintIndex,
}

impl Default for PbsPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl PbsPlanner {
    pub fn new() -> Self {
        Self {
            conflict_grid: ConflictGrid::new(),
            ci_buf: FlatConstraintIndex::new(1, 1, 1),
            stg: SpacetimeGrid::new(),
            seq_stg: SeqGoalGrid::new(),
            empty_ci: FlatConstraintIndex::new(1, 1, 1),
        }
    }
}

impl WindowedPlanner for PbsPlanner {
    fn name(&self) -> &'static str {
        "pbs"
    }

    fn plan_window(
        &mut self,
        ctx: &WindowContext,
        _rng: &mut SeededRng,
    ) -> WindowResult {
        let n = ctx.agents.len();
        if n == 0 {
            return WindowResult::Solved(Vec::new());
        }

        let cells = (ctx.grid.width * ctx.grid.height) as usize;
        self.conflict_grid.ensure_size(cells);

        // Initial plans: each agent plans independently.
        // Agents with goal sequences use sequential A*; others use BFS-guided A*.
        let mut initial_plans: Vec<Vec<Action>> = Vec::with_capacity(n);

        self.empty_ci.reset(ctx.grid.width, ctx.grid.height, ctx.horizon as u64);

        for i in 0..n {
            let agent = &ctx.agents[i];
            let plan = if agent.goal_sequence.is_empty() {
                spacetime_astar_guided(
                    ctx.grid, agent.pos, agent.goal, i,
                    &Constraints::new(), ctx.horizon as u64,
                    Some(ctx.distance_maps[i]),
                )
            } else {
                let seq_dms: Vec<DistanceMap> = agent.goal_sequence.iter()
                    .map(|&g| DistanceMap::compute(ctx.grid, g))
                    .collect();
                let mut goals: Vec<(IVec2, &DistanceMap)> = vec![(agent.goal, ctx.distance_maps[i])];
                for (j, &g) in agent.goal_sequence.iter().enumerate() {
                    goals.push((g, &seq_dms[j]));
                }
                // Try sequential A*; fall back progressively: drop trailing
                // goals until we find a feasible subset, then single-goal.
                let mut plan_result = Err(super::traits::SolverError::NoSolution);
                while goals.len() > 1 {
                    plan_result = spacetime_astar_sequential(
                        ctx.grid, agent.pos, &goals, &self.empty_ci,
                        ctx.horizon as u64, &mut self.seq_stg, u64::MAX,
                    );
                    if plan_result.is_ok() { break; }
                    goals.pop();
                }
                if plan_result.is_err() {
                    plan_result = spacetime_astar_guided(
                        ctx.grid, agent.pos, agent.goal, i,
                        &Constraints::new(), ctx.horizon as u64,
                        Some(ctx.distance_maps[i]),
                    );
                }
                plan_result
            };
            match plan {
                Ok(p) => initial_plans.push(p),
                Err(_) => initial_plans.push(vec![Action::Wait; ctx.horizon.min(1)]),
            }
        }

        // Build timelines once for initial plans
        let initial_timelines = build_timelines(&initial_plans, ctx.agents);

        // PBS tree search — DFS with best-node tracking
        let mut dfs: Vec<PbsNode> = Vec::new();
        let mut node_count = 0usize;
        let mut best_node: Option<PbsNode> = None;

        // Build root node with earliest_collision
        let root_conflicts = self.conflict_grid.count_conflicts(&initial_timelines, ctx.grid.width, ctx.horizon);
        let root_earliest = self.conflict_grid.detect_first(&initial_timelines, ctx.grid.width, ctx.horizon)
            .map(|c| c.time as usize)
            .unwrap_or(usize::MAX);

        dfs.push(PbsNode {
            plans: initial_plans,
            timelines: initial_timelines,
            priority_pairs: Vec::new(),
            conflicts: root_conflicts,
            earliest_collision: root_earliest,
            id: node_count,
        });
        node_count += 1;

        while let Some(node) = dfs.pop() {
            if node_count >= ctx.node_limit {
                let best = best_node.unwrap_or(node);
                return to_partial_result(best.plans, ctx.agents);
            }

            // Update best node (prefer later earliest_collision, tie-break on fewer conflicts)
            let dominated = match &best_node {
                Some(bn) => node.earliest_collision > bn.earliest_collision
                    || (node.earliest_collision == bn.earliest_collision && node.conflicts < bn.conflicts),
                None => true,
            };
            if dominated {
                best_node = Some(PbsNode {
                    plans: node.plans.clone(),
                    timelines: node.timelines.clone(),
                    priority_pairs: node.priority_pairs.clone(),
                    conflicts: node.conflicts,
                    earliest_collision: node.earliest_collision,
                    id: node.id,
                });
            }

            if let Some(conflict) = self.conflict_grid.detect_first(&node.timelines, ctx.grid.width, ctx.horizon) {
                let child1 = try_branch(
                    &node, conflict.agent_a, conflict.agent_b,
                    ctx.agents, ctx.grid, ctx.horizon, ctx.distance_maps,
                    &mut self.ci_buf, &mut self.stg,
                );
                let child2 = try_branch(
                    &node, conflict.agent_b, conflict.agent_a,
                    ctx.agents, ctx.grid, ctx.horizon, ctx.distance_maps,
                    &mut self.ci_buf, &mut self.stg,
                );

                // Push worse child first, better child second (better popped first in DFS)
                let mut children: Vec<PbsNode> = Vec::new();
                for child_opt in [child1, child2] {
                    if let Some(mut child) = child_opt {
                        let c_conflicts = self.conflict_grid.count_conflicts(&child.timelines, ctx.grid.width, ctx.horizon);
                        let c_earliest = self.conflict_grid.detect_first(&child.timelines, ctx.grid.width, ctx.horizon)
                            .map(|c| c.time as usize)
                            .unwrap_or(usize::MAX);
                        child.conflicts = c_conflicts;
                        child.earliest_collision = c_earliest;
                        child.id = node_count;
                        node_count += 1;
                        children.push(child);
                    }
                }

                // Sort: worse first (bottom of stack), better second (top = explored first)
                children.sort_by(|a, b| {
                    a.conflicts.cmp(&b.conflicts)
                        .then_with(|| a.earliest_collision.cmp(&b.earliest_collision).reverse())
                });
                for child in children {
                    dfs.push(child);
                }
            } else {
                // No conflicts — solution found
                return to_window_result(node.plans, ctx.agents);
            }
        }

        // No solution found — return best partial
        match best_node {
            Some(best) => to_partial_result(best.plans, ctx.agents),
            None => WindowResult::Partial { solved: Vec::new(), failed: (0..n).collect() },
        }
    }
}

/// Check if adding edge (higher → lower) to the priority pairs creates a cycle.
fn would_create_cycle(pairs: &[(usize, usize)], higher: usize, lower: usize) -> bool {
    let mut stack = vec![lower];
    let mut visited = vec![false; pairs.len().max(higher + 1).max(lower + 1)];
    visited[lower] = true;

    while let Some(node) = stack.pop() {
        for &(h, l) in pairs {
            if h == node && !visited.get(l).copied().unwrap_or(false) {
                if l == higher {
                    return true;
                }
                if l < visited.len() {
                    visited[l] = true;
                    stack.push(l);
                }
            }
        }
    }
    false
}

fn try_branch(
    parent: &PbsNode,
    higher: usize,
    lower: usize,
    agents: &[WindowAgent],
    grid: &GridMap,
    horizon: usize,
    distance_maps: &[&DistanceMap],
    ci_buf: &mut FlatConstraintIndex,
    stg: &mut SpacetimeGrid,
) -> Option<PbsNode> {
    if would_create_cycle(&parent.priority_pairs, higher, lower) {
        return None;
    }

    let mut new_pairs = parent.priority_pairs.clone();
    new_pairs.push((higher, lower));

    let mut new_plans = parent.plans.clone();

    let dm = distance_maps.get(lower).copied();
    if let Some(new_plan) = plan_agent(lower, agents, &new_plans, &new_pairs, grid, horizon, dm, ci_buf, stg) {
        new_plans[lower] = new_plan;
        let mut new_timelines = parent.timelines.clone();
        rebuild_timeline(&mut new_timelines, &new_plans, agents, lower);
        Some(PbsNode {
            plans: new_plans,
            timelines: new_timelines,
            priority_pairs: new_pairs,
            conflicts: 0,
            earliest_collision: 0,
            id: 0,
        })
    } else {
        None
    }
}

fn to_window_result(plans: Vec<Vec<Action>>, agents: &[WindowAgent]) -> WindowResult {
    let fragments: Vec<PlanFragment> = plans
        .into_iter()
        .zip(agents.iter())
        .map(|(plan, agent)| PlanFragment {
            agent_index: agent.index,
            actions: plan.into_iter().collect(),
        })
        .collect();
    WindowResult::Solved(fragments)
}

fn to_partial_result(plans: Vec<Vec<Action>>, agents: &[WindowAgent]) -> WindowResult {
    let mut solved = Vec::new();
    let mut failed = Vec::new();

    for (plan, agent) in plans.into_iter().zip(agents.iter()) {
        if plan.is_empty() || plan.iter().all(|a| *a == Action::Wait) {
            failed.push(agent.index);
        } else {
            solved.push(PlanFragment {
                agent_index: agent.index,
                actions: plan.into_iter().collect(),
            });
        }
    }

    if failed.is_empty() {
        WindowResult::Solved(solved)
    } else {
        WindowResult::Partial { solved, failed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;
    use crate::core::grid::GridMap;
    use crate::core::seed::SeededRng;
    use crate::solver::heuristics::DistanceMap;

    fn make_ctx<'a>(
        grid: &'a GridMap,
        agents: &'a [WindowAgent],
        dist_maps: &'a [&'a DistanceMap],
    ) -> WindowContext<'a> {
        WindowContext {
            grid,
            horizon: 20,
            node_limit: 500,
            agents,
            distance_maps: dist_maps,
        }
    }

    #[test]
    fn cycle_detection_no_cycle() {
        let pairs = vec![(0, 1), (1, 2)];
        assert!(!would_create_cycle(&pairs, 0, 2));
        assert!(!would_create_cycle(&pairs, 2, 3));
    }

    #[test]
    fn cycle_detection_direct_cycle() {
        let pairs = vec![(0, 1)];
        assert!(would_create_cycle(&pairs, 1, 0));
    }

    #[test]
    fn cycle_detection_transitive_cycle() {
        let pairs = vec![(0, 1), (1, 2)];
        assert!(would_create_cycle(&pairs, 2, 0));
    }

    #[test]
    fn cycle_detection_empty_pairs() {
        assert!(!would_create_cycle(&[], 0, 1));
    }

    #[test]
    fn pbs_empty_agents() {
        let grid = GridMap::new(5, 5);
        let ctx = make_ctx(&grid, &[], &[]);
        let mut planner = PbsPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        assert!(matches!(result, WindowResult::Solved(v) if v.is_empty()));
    }

    #[test]
    fn pbs_single_agent_reaches_goal() {
        let grid = GridMap::new(5, 5);
        let agents = vec![WindowAgent { index: 0, pos: IVec2::ZERO, goal: IVec2::new(4, 4), goal_sequence: SmallVec::new() }];
        let dm = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm];
        let ctx = make_ctx(&grid, &agents, &dist_maps);
        let mut planner = PbsPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        match result {
            WindowResult::Solved(frags) => {
                assert_eq!(frags.len(), 1);
                assert!(!frags[0].actions.is_empty());
            }
            _ => panic!("expected Solved"),
        }
    }

    #[test]
    fn pbs_two_agents_no_conflict() {
        let grid = GridMap::new(5, 5);
        let agents = vec![
            WindowAgent { index: 0, pos: IVec2::new(0, 0), goal: IVec2::new(4, 0), goal_sequence: SmallVec::new() },
            WindowAgent { index: 1, pos: IVec2::new(0, 4), goal: IVec2::new(4, 4), goal_sequence: SmallVec::new() },
        ];
        let dm0 = DistanceMap::compute(&grid, IVec2::new(4, 0));
        let dm1 = DistanceMap::compute(&grid, IVec2::new(4, 4));
        let dist_maps: Vec<&DistanceMap> = vec![&dm0, &dm1];
        let ctx = make_ctx(&grid, &agents, &dist_maps);
        let mut planner = PbsPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        assert!(matches!(result, WindowResult::Solved(_)));
    }

    #[test]
    fn conflict_grid_detects_vertex_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5×5

        // Two agents at same position at t=1
        let timelines = vec![
            vec![IVec2::new(0, 0), IVec2::new(1, 0)],
            vec![IVec2::new(2, 0), IVec2::new(1, 0)],
        ];
        let conflict = cg.detect_first(&timelines, 5, usize::MAX);
        assert!(conflict.is_some());
    }

    #[test]
    fn conflict_grid_detects_edge_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5×5

        // Two agents swap positions
        let timelines = vec![
            vec![IVec2::new(0, 0), IVec2::new(1, 0)],
            vec![IVec2::new(1, 0), IVec2::new(0, 0)],
        ];
        let conflict = cg.detect_first(&timelines, 5, usize::MAX);
        assert!(conflict.is_some());
    }

    #[test]
    fn conflict_grid_no_conflict() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25);

        let timelines = vec![
            vec![IVec2::new(0, 0), IVec2::new(1, 0)],
            vec![IVec2::new(0, 4), IVec2::new(1, 4)],
        ];
        assert!(cg.detect_first(&timelines, 5, usize::MAX).is_none());
    }

    #[test]
    fn pbs_finds_solution_with_tight_node_limit() {
        let grid = GridMap::new(5, 5);
        let agents = vec![
            WindowAgent { index: 0, pos: IVec2::new(0, 2), goal: IVec2::new(4, 2), goal_sequence: SmallVec::new() },
            WindowAgent { index: 1, pos: IVec2::new(4, 2), goal: IVec2::new(0, 2), goal_sequence: SmallVec::new() },
        ];
        let dm0 = DistanceMap::compute(&grid, IVec2::new(4, 2));
        let dm1 = DistanceMap::compute(&grid, IVec2::new(0, 2));
        let dist_maps: Vec<&DistanceMap> = vec![&dm0, &dm1];
        let ctx = WindowContext {
            grid: &grid,
            horizon: 12,
            node_limit: 6,
            agents: &agents,
            distance_maps: &dist_maps,
        };
        let mut planner = PbsPlanner::new();
        let mut rng = SeededRng::new(42);
        let result = planner.plan_window(&ctx, &mut rng);
        assert!(matches!(result, WindowResult::Solved(_)),
            "DFS should solve 2 crossing agents within 6 nodes");
    }

    #[test]
    fn conflict_grid_respects_window_scope() {
        let mut cg = ConflictGrid::new();
        cg.ensure_size(25); // 5x5

        // Two agents that collide at t=2 (both at position (2,0))
        let timelines = vec![
            vec![IVec2::new(0,0), IVec2::new(1,0), IVec2::new(2,0)],
            vec![IVec2::new(4,0), IVec2::new(3,0), IVec2::new(2,0)],
        ];
        // Full window: should detect conflict at t=2
        assert!(cg.detect_first(&timelines, 5, usize::MAX).is_some());
        // Window=1: only check t=0..1, no conflict
        assert!(cg.detect_first(&timelines, 5, 1).is_none());
    }
}
