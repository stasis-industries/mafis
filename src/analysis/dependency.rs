use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::constants;
use crate::core::agent::{AgentIndex, LogicalAgent};
use crate::core::live_sim::LiveSim;
use crate::core::runner::SimAgent;
use crate::core::state::SimulationConfig;
use crate::fault::breakdown::Dead;

/// Tracks the last tick the ADG was computed, enabling tick-stride throttling.
#[derive(Resource, Debug, Default)]
pub struct AdgThrottle {
    pub last_tick: u64,
}

/// Return the ADG tick stride for a given agent count.
pub fn adg_stride(agent_count: usize) -> u64 {
    if agent_count <= constants::ADG_TIER_SMALL {
        constants::ADG_STRIDE_SMALL
    } else if agent_count <= constants::ADG_TIER_MED {
        constants::ADG_STRIDE_MED
    } else if agent_count <= constants::ADG_TIER_LARGE {
        constants::ADG_STRIDE_LARGE
    } else {
        constants::ADG_STRIDE_XLARGE
    }
}

/// Directed graph: edge A→B means agent B depends on agent A
/// (B's future path crosses a tile A currently occupies).
#[derive(Resource, Debug, Default)]
pub struct ActionDependencyGraph {
    /// Forward edges: entity → entities that depend on it
    pub dependents: HashMap<Entity, Vec<Entity>>,
    /// Reverse edges: entity → entities it depends on
    pub dependencies: HashMap<Entity, Vec<Entity>>,
    /// Current tile occupation: pos → entity
    pub occupation: HashMap<IVec2, Entity>,
}

impl ActionDependencyGraph {
    pub fn clear(&mut self) {
        self.dependents.clear();
        self.dependencies.clear();
        self.occupation.clear();
    }

    /// Clear and pre-allocate for the expected agent count.
    /// Avoids repeated reallocation during the tick.
    pub fn clear_with_capacity(&mut self, agent_count: usize) {
        self.dependents.clear();
        self.dependents.reserve(agent_count);
        self.dependencies.clear();
        self.dependencies.reserve(agent_count);
        self.occupation.clear();
        self.occupation.reserve(agent_count);
    }

    pub fn direct_dependents(&self, entity: Entity) -> &[Entity] {
        self.dependents.get(&entity).map_or(&[], |v| v.as_slice())
    }
}

/// Rebuild the ADG from current positions and planned paths.
/// Uses tick-stride throttling: skips computation on off-ticks for large agent counts.
pub fn build_adg(
    agents: Query<(Entity, &AgentIndex, &LogicalAgent), Without<Dead>>,
    sim: Option<Res<LiveSim>>,
    mut adg: ResMut<ActionDependencyGraph>,
    mut seen_edges: Local<HashSet<Entity>>,
    sim_config: Res<SimulationConfig>,
    mut throttle: ResMut<AdgThrottle>,
) {
    let agent_count = agents.iter().len();
    let stride = adg_stride(agent_count);

    // Throttle: skip this tick if not enough ticks elapsed since last computation
    if sim_config.tick.saturating_sub(throttle.last_tick) < stride && throttle.last_tick > 0 {
        return;
    }
    throttle.last_tick = sim_config.tick;

    adg.clear_with_capacity(agent_count);

    // 1. Build occupation map: current_pos → entity
    for (entity, _, agent) in &agents {
        adg.occupation.insert(agent.current_pos, entity);
    }

    // 2. For each agent B, check if its next few planned steps
    //    cross a tile currently occupied by another agent A.
    //    If so, add edge A→B (B depends on A moving out of the way).
    for (entity_b, idx_b, agent_b) in &agents {
        let mut pos = agent_b.current_pos;
        seen_edges.clear();

        // Read planned path from runner (zero-copy) when available,
        // fall back to ECS planned_path (rewind/legacy).
        let runner_path =
            sim.as_ref().and_then(|s| s.runner.agents.get(idx_b.0)).map(|sa| &sa.planned_path);

        let path_iter: Box<dyn Iterator<Item = _>> = if let Some(rp) = runner_path {
            Box::new(rp.iter().take(constants::ADG_LOOKAHEAD))
        } else {
            Box::new(agent_b.planned_path.iter().take(constants::ADG_LOOKAHEAD))
        };

        for action in path_iter {
            pos = action.apply(pos);

            if let Some(&entity_a) = adg.occupation.get(&pos)
                && entity_a != entity_b
                && seen_edges.insert(entity_a)
            {
                adg.dependents.entry(entity_a).or_default().push(entity_b);
                adg.dependencies.entry(entity_b).or_default().push(entity_a);
            }
        }
    }
}

/// Betweenness centrality scores computed periodically via Brandes algorithm.
#[derive(Resource, Debug, Default)]
pub struct BetweennessCriticality {
    pub scores: HashMap<Entity, f32>,
    pub last_tick: u64,
}

impl BetweennessCriticality {
    pub fn clear(&mut self) {
        self.scores.clear();
        self.last_tick = 0;
    }
}

/// Periodic betweenness centrality via Brandes algorithm on the ADG.
/// Only runs every BETWEENNESS_INTERVAL ticks and below BETWEENNESS_AGENT_LIMIT agents.
pub fn compute_betweenness_criticality(
    agents: Query<Entity, (With<LogicalAgent>, Without<Dead>)>,
    adg: Res<ActionDependencyGraph>,
    sim_config: Res<SimulationConfig>,
    mut betweenness: ResMut<BetweennessCriticality>,
) {
    let agent_count = agents.iter().len();

    // Guard: interval and agent limit
    if constants::BETWEENNESS_INTERVAL == 0 {
        return;
    }
    if agent_count > constants::BETWEENNESS_AGENT_LIMIT {
        return;
    }
    if !sim_config.tick.is_multiple_of(constants::BETWEENNESS_INTERVAL) {
        return;
    }

    betweenness.scores.clear();
    betweenness.last_tick = sim_config.tick;

    // Collect all entities in the ADG
    let entities: Vec<Entity> = agents.iter().collect();
    if entities.is_empty() {
        return;
    }

    let entity_to_idx: HashMap<Entity, usize> =
        entities.iter().enumerate().map(|(i, &e)| (e, i)).collect();
    let n = entities.len();

    let mut cb = vec![0.0f32; n]; // betweenness accumulator

    // Pre-allocate buffers outside the loop (avoids n² Vec allocations).
    let mut stack: Vec<usize> = Vec::with_capacity(n);
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut sigma = vec![0.0f32; n];
    let mut dist = vec![-1i32; n];
    let mut delta = vec![0.0f32; n];
    let mut queue = VecDeque::with_capacity(n);

    // Brandes algorithm: BFS from each source
    for s_idx in 0..n {
        // Reset buffers (O(n) clear instead of O(n) alloc)
        stack.clear();
        for p in predecessors.iter_mut() {
            p.clear();
        }
        sigma.fill(0.0);
        dist.fill(-1);
        delta.fill(0.0);
        queue.clear();

        sigma[s_idx] = 1.0;
        dist[s_idx] = 0;
        queue.push_back(s_idx);

        while let Some(v_idx) = queue.pop_front() {
            stack.push(v_idx);
            let v = entities[v_idx];

            // Iterate over ADG neighbors (dependents = forward edges)
            if let Some(neighbors) = adg.dependents.get(&v) {
                for &w in neighbors {
                    if let Some(&w_idx) = entity_to_idx.get(&w) {
                        if dist[w_idx] < 0 {
                            dist[w_idx] = dist[v_idx] + 1;
                            queue.push_back(w_idx);
                        }
                        if dist[w_idx] == dist[v_idx] + 1 {
                            sigma[w_idx] += sigma[v_idx];
                            predecessors[w_idx].push(v_idx);
                        }
                    }
                }
            }
        }

        // Back-propagation
        while let Some(w_idx) = stack.pop() {
            for &v_idx in &predecessors[w_idx] {
                let frac = (sigma[v_idx] / sigma[w_idx]) * (1.0 + delta[w_idx]);
                delta[v_idx] += frac;
            }
            if w_idx != s_idx {
                cb[w_idx] += delta[w_idx];
            }
        }
    }

    // Normalize by (n-1)*(n-2) for directed graphs, store as scores
    let norm = if n > 2 { ((n - 1) * (n - 2)) as f32 } else { 1.0 };
    for (i, &e) in entities.iter().enumerate() {
        let normalized = cb[i] / norm;
        if normalized > 0.0 {
            betweenness.scores.insert(e, normalized);
        }
    }
}

// ---------------------------------------------------------------------------
// Headless (index-based) ADG — for experiment runner
// ---------------------------------------------------------------------------

/// Index-based dependency graph for headless use (no Bevy Entity).
/// Used by the experiment runner for cascade analysis.
pub struct IndexedDependencyGraph {
    pub(crate) dependents: HashMap<usize, Vec<usize>>,
}

impl IndexedDependencyGraph {
    /// Get the list of agents that depend on agent `idx`.
    pub fn direct_dependents(&self, idx: usize) -> &[usize] {
        self.dependents.get(&idx).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Build an index-based ADG from SimAgent positions and planned paths.
/// Mirrors the logic of the ECS `build_adg` system but uses agent indices
/// instead of Bevy entities.
pub fn build_adg_from_agents(agents: &[SimAgent], lookahead: usize) -> IndexedDependencyGraph {
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut occupation: HashMap<IVec2, usize> = HashMap::with_capacity(agents.len());

    // Build occupation map: pos → agent index (alive only)
    for (i, agent) in agents.iter().enumerate() {
        if agent.alive {
            occupation.insert(agent.pos, i);
        }
    }

    // For each alive agent B, walk planned_path and find dependencies
    let mut seen = HashSet::new();
    for (idx_b, agent_b) in agents.iter().enumerate() {
        if !agent_b.alive {
            continue;
        }
        seen.clear();
        let mut pos = agent_b.pos;
        for action in agent_b.planned_path.iter().take(lookahead) {
            pos = action.apply(pos);
            if let Some(&idx_a) = occupation.get(&pos) {
                if idx_a != idx_b && seen.insert(idx_a) {
                    dependents.entry(idx_a).or_default().push(idx_b);
                }
            }
        }
    }

    IndexedDependencyGraph { dependents }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("valid entity index")
    }

    // ── Default / clear ───────────────────────────────────────────────────

    #[test]
    fn default_adg_is_empty() {
        let adg = ActionDependencyGraph::default();
        assert!(adg.dependents.is_empty());
        assert!(adg.dependencies.is_empty());
        assert!(adg.occupation.is_empty());
    }

    #[test]
    fn clear_empties_all_maps() {
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        let eb = make_entity(2);

        adg.occupation.insert(IVec2::new(0, 0), ea);
        adg.dependents.entry(ea).or_default().push(eb);
        adg.dependencies.entry(eb).or_default().push(ea);

        adg.clear();

        assert!(adg.dependents.is_empty());
        assert!(adg.dependencies.is_empty());
        assert!(adg.occupation.is_empty());
    }

    #[test]
    fn clear_with_capacity_empties_all_maps() {
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        adg.occupation.insert(IVec2::new(1, 2), ea);
        adg.dependents.entry(ea).or_default().push(make_entity(2));

        adg.clear_with_capacity(50);

        assert!(adg.dependents.is_empty());
        assert!(adg.dependencies.is_empty());
        assert!(adg.occupation.is_empty());
    }

    #[test]
    fn clear_with_capacity_zero_is_valid() {
        let mut adg = ActionDependencyGraph::default();
        adg.clear_with_capacity(0); // must not panic
        assert!(adg.dependents.is_empty());
    }

    // ── direct_dependents ────────────────────────────────────────────────

    #[test]
    fn direct_dependents_returns_empty_slice_for_unknown_entity() {
        let adg = ActionDependencyGraph::default();
        let e = make_entity(99);
        assert_eq!(adg.direct_dependents(e), &[]);
    }

    #[test]
    fn direct_dependents_returns_correct_dependents() {
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        let eb = make_entity(2);
        let ec = make_entity(3);

        // ea blocks eb and ec
        adg.dependents.entry(ea).or_default().push(eb);
        adg.dependents.entry(ea).or_default().push(ec);

        let deps = adg.direct_dependents(ea);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&eb));
        assert!(deps.contains(&ec));
    }

    #[test]
    fn direct_dependents_is_directional() {
        // ea→eb exists, but eb has no dependents
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        let eb = make_entity(2);

        adg.dependents.entry(ea).or_default().push(eb);

        assert_eq!(adg.direct_dependents(ea).len(), 1);
        assert_eq!(adg.direct_dependents(eb), &[]);
    }

    // ── Occupation map ────────────────────────────────────────────────────

    #[test]
    fn occupation_maps_position_to_entity() {
        let mut adg = ActionDependencyGraph::default();
        let e = make_entity(7);
        let pos = IVec2::new(3, 4);
        adg.occupation.insert(pos, e);

        assert_eq!(adg.occupation.get(&pos), Some(&e));
        assert_eq!(adg.occupation.get(&IVec2::new(0, 0)), None);
    }

    #[test]
    fn occupation_overwrite_replaces_entity() {
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        let eb = make_entity(2);
        let pos = IVec2::new(1, 1);

        adg.occupation.insert(pos, ea);
        adg.occupation.insert(pos, eb); // overwrite

        assert_eq!(adg.occupation.get(&pos), Some(&eb));
    }

    // ── Graph bidirectionality ────────────────────────────────────────────

    #[test]
    fn both_forward_and_reverse_edges_can_be_stored() {
        let mut adg = ActionDependencyGraph::default();
        let ea = make_entity(1);
        let eb = make_entity(2);

        // Simulate what build_adg does: A→B means B depends on A
        adg.dependents.entry(ea).or_default().push(eb); // forward: ea's dependents
        adg.dependencies.entry(eb).or_default().push(ea); // reverse: eb depends on ea

        assert!(adg.direct_dependents(ea).contains(&eb));
        assert!(adg.dependencies.get(&eb).unwrap().contains(&ea));
    }

    #[test]
    fn multiple_clear_calls_are_safe() {
        let mut adg = ActionDependencyGraph::default();
        adg.clear();
        adg.clear();
        assert!(adg.dependents.is_empty());
    }

    // ── AdgThrottle / stride ─────────────────────────────────────────────

    #[test]
    fn adg_stride_small_agents() {
        assert_eq!(adg_stride(1), constants::ADG_STRIDE_SMALL);
        assert_eq!(adg_stride(50), constants::ADG_STRIDE_SMALL);
        assert_eq!(adg_stride(100), constants::ADG_STRIDE_SMALL);
    }

    #[test]
    fn adg_stride_med_agents() {
        assert_eq!(adg_stride(101), constants::ADG_STRIDE_MED);
        assert_eq!(adg_stride(200), constants::ADG_STRIDE_MED);
        assert_eq!(adg_stride(300), constants::ADG_STRIDE_MED);
    }

    #[test]
    fn adg_stride_large_agents() {
        assert_eq!(adg_stride(301), constants::ADG_STRIDE_LARGE);
        assert_eq!(adg_stride(500), constants::ADG_STRIDE_LARGE);
    }

    #[test]
    fn adg_stride_xlarge_agents() {
        assert_eq!(adg_stride(501), constants::ADG_STRIDE_XLARGE);
        assert_eq!(adg_stride(1000), constants::ADG_STRIDE_XLARGE);
    }

    #[test]
    fn adg_throttle_defaults_to_zero() {
        let t = AdgThrottle::default();
        assert_eq!(t.last_tick, 0);
    }

    #[test]
    fn betweenness_criticality_defaults_empty() {
        let b = BetweennessCriticality::default();
        assert!(b.scores.is_empty());
        assert_eq!(b.last_tick, 0);
    }

    #[test]
    fn betweenness_criticality_clear_resets() {
        let mut b = BetweennessCriticality::default();
        b.scores.insert(make_entity(1), 0.5);
        b.last_tick = 42;
        b.clear();
        assert!(b.scores.is_empty());
        assert_eq!(b.last_tick, 0);
    }

    // ── IndexedDependencyGraph (headless) ─────────────────────────────

    #[test]
    fn test_indexed_adg_empty() {
        let agents: Vec<SimAgent> = vec![];
        let adg = build_adg_from_agents(&agents, 3);
        assert!(adg.dependents.is_empty());
    }

    #[test]
    fn test_indexed_adg_linear_chain() {
        use crate::core::action::Action;
        use std::collections::VecDeque;
        // Agent A at (0,0), no plan. Agent B at (1,0), plans to move West to (0,0).
        let mut agent_a = SimAgent::new(IVec2::new(0, 0));
        agent_a.alive = true;
        let mut agent_b = SimAgent::new(IVec2::new(1, 0));
        agent_b.alive = true;
        agent_b.planned_path =
            VecDeque::from(vec![Action::Move(crate::core::action::Direction::West)]);

        let adg = build_adg_from_agents(&[agent_a, agent_b], 3);
        // B depends on A (B's path crosses A's position)
        assert_eq!(adg.direct_dependents(0), &[1]); // A's dependents = [B]
        assert!(adg.direct_dependents(1).is_empty()); // B has no dependents
    }

    #[test]
    fn test_indexed_adg_dead_excluded() {
        use crate::core::action::Action;
        use std::collections::VecDeque;
        // Dead agent's position should not appear in occupation map
        let mut agent_a = SimAgent::new(IVec2::new(0, 0));
        agent_a.alive = false; // dead
        let mut agent_b = SimAgent::new(IVec2::new(1, 0));
        agent_b.alive = true;
        agent_b.planned_path =
            VecDeque::from(vec![Action::Move(crate::core::action::Direction::West)]);

        let adg = build_adg_from_agents(&[agent_a, agent_b], 3);
        // A is dead, so no occupation → no edge
        assert!(adg.dependents.is_empty());
    }
}
