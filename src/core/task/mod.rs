use bevy::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashSet, VecDeque};

use super::topology::ZoneMap;
use crate::constants::THROUGHPUT_WINDOW_SIZE;
use crate::solver::heuristics::DistanceMapCache;

pub mod closest;
pub use closest::ClosestFirstScheduler;
pub mod random;
pub use random::{RandomScheduler, random_cell_from};
pub mod recycle;
pub use recycle::{RecycleResult, TaskAgentSnapshot, TaskUpdate, recycle_goals_core};

// ---------------------------------------------------------------------------
// TaskLeg
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Default)]
pub enum TaskLeg {
    #[default]
    Free,
    /// Heading to pickup zone to collect cargo (empty travel / deadheading).
    TravelEmpty(IVec2),
    /// At pickup zone, loading cargo (0-tick dwell for now).
    Loading(IVec2),
    /// Traveling to the back of a delivery queue line.
    /// `from` = pickup cell, `to` = delivery cell, `line_index` = queue line index.
    TravelToQueue { from: IVec2, to: IVec2, line_index: usize },
    /// Physically waiting in a delivery queue slot, shuffling forward each tick.
    /// `from` = pickup cell, `to` = delivery cell, `line_index` = queue line index.
    Queuing { from: IVec2, to: IVec2, line_index: usize },
    /// Carrying cargo to delivery zone (loaded travel).
    TravelLoaded { from: IVec2, to: IVec2 },
    /// At delivery zone, unloading cargo.
    /// Roadmap: add configurable dwell time (e.g. 3-5 ticks) to model real
    /// unloading duration. Currently unused — transition skips straight from
    /// TravelLoaded → Free.
    Unloading { from: IVec2, to: IVec2 },
    /// Agent parked at charging station.
    /// Roadmap: add battery/energy model with configurable charge rate and
    /// threshold-based routing to charging stations. Currently unused.
    Charging,
}

impl TaskLeg {
    pub fn label(&self) -> &'static str {
        match self {
            TaskLeg::Free => "free",
            TaskLeg::TravelEmpty(_) => "travel_empty",
            TaskLeg::Loading(_) => "loading",
            TaskLeg::TravelToQueue { .. } => "travel_to_queue",
            TaskLeg::Queuing { .. } => "queuing",
            TaskLeg::TravelLoaded { .. } => "travel_loaded",
            TaskLeg::Unloading { .. } => "unloading",
            TaskLeg::Charging => "charging",
        }
    }

    /// Index into the 2D task-heat palette (0..=6).
    pub fn palette_index(&self) -> usize {
        match self {
            TaskLeg::Free => 0,
            TaskLeg::TravelEmpty(_) => 1,
            TaskLeg::Loading(_) => 2,
            TaskLeg::TravelToQueue { .. } => 3,
            TaskLeg::Queuing { .. } => 4,
            TaskLeg::TravelLoaded { .. } => 5,
            TaskLeg::Unloading { .. } => 6,
            TaskLeg::Charging => 7,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskScheduler trait
// ---------------------------------------------------------------------------

pub trait TaskScheduler: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2>;

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2>;

    /// Batch-assign pickup tasks to a set of free agents.
    ///
    /// # Semantics
    ///
    /// Task *creation* is always uniform-random: tasks are drawn from all
    /// available pickup cells without positional bias. This method controls
    /// only task *assignment*: which randomly-created task each agent receives.
    ///
    /// This separation ensures that "closest" means "among the randomly-available
    /// tasks, pick the one nearest to me" — not "always go to the globally
    /// nearest cell", which causes every agent to converge on the same hotspot.
    ///
    /// # Arguments
    /// - `free_agents`: `(agent_index, current_pos)` for each agent needing a task.
    /// - `used_goals`: cells already claimed; updated in-place as tasks are assigned.
    ///
    /// # Returns
    /// `(agent_index, assigned_pickup_cell)` for each successfully assigned agent.
    ///
    /// Default: calls `assign_pickup` per-agent sequentially (correct for
    /// `RandomScheduler` which already picks diversely).
    fn assign_pickups_batch(
        &self,
        free_agents: &[(usize, IVec2)],
        zones: &ZoneMap,
        used_goals: &mut HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Vec<(usize, IVec2)> {
        let mut assignments = Vec::new();
        for &(idx, pos) in free_agents {
            if let Some(pickup) = self.assign_pickup(zones, pos, used_goals, rng) {
                used_goals.insert(pickup);
                assignments.push((idx, pickup));
            }
        }
        assignments
    }
}

// ---------------------------------------------------------------------------
// ActiveScheduler resource
// ---------------------------------------------------------------------------

pub const SCHEDULER_NAMES: &[(&str, &str)] = &[("random", "Random"), ("closest", "Closest")];

#[derive(Resource)]
pub struct ActiveScheduler {
    scheduler: Box<dyn TaskScheduler>,
    name: String,
}

impl ActiveScheduler {
    pub fn from_name(name: &str) -> Self {
        let scheduler: Box<dyn TaskScheduler> = match name {
            "random" => Box::new(RandomScheduler),
            "closest" => Box::new(ClosestFirstScheduler),
            _ => Box::new(RandomScheduler),
        };
        let actual_name = scheduler.name().to_string();
        Self { scheduler, name: actual_name }
    }

    pub fn scheduler(&self) -> &dyn TaskScheduler {
        self.scheduler.as_ref()
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Default for ActiveScheduler {
    fn default() -> Self {
        Self::from_name("random")
    }
}

// ---------------------------------------------------------------------------
// LifelongConfig resource
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct LifelongConfig {
    pub enabled: bool,
    pub tasks_completed: u64,
    pub needs_replan: bool,
    /// Tick numbers at which tasks were completed (for throughput calculation).
    completion_ticks: VecDeque<u64>,
    /// Cached: tick number of the most recent completion for O(1) throughput.
    last_completion_tick: u64,
    /// Cached: number of completions at `last_completion_tick`.
    last_completion_count: u64,
}

impl Default for LifelongConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tasks_completed: 0,
            needs_replan: false,
            completion_ticks: VecDeque::new(),
            last_completion_tick: 0,
            last_completion_count: 0,
        }
    }
}

impl LifelongConfig {
    pub fn reset(&mut self) {
        self.tasks_completed = 0;
        self.needs_replan = false;
        self.completion_ticks.clear();
        self.last_completion_tick = 0;
        self.last_completion_count = 0;
    }

    /// Restore to a specific snapshot state. Used after rewind to restore
    /// deterministic task scheduling from the snapshotted completion count.
    pub fn restore_from_snapshot(&mut self, tasks_completed: u64, completion_ticks: VecDeque<u64>) {
        self.tasks_completed = tasks_completed;
        // Rebuild throughput cache from the restored ticks
        if let Some(&last_tick) = completion_ticks.back() {
            self.last_completion_tick = last_tick;
            self.last_completion_count =
                completion_ticks.iter().rev().take_while(|&&t| t == last_tick).count() as u64;
        } else {
            self.last_completion_tick = 0;
            self.last_completion_count = 0;
        }
        self.completion_ticks = completion_ticks;
        self.needs_replan = true;
    }

    /// Read-only access to the completion_ticks window (for snapshotting).
    pub fn completion_ticks(&self) -> &VecDeque<u64> {
        &self.completion_ticks
    }

    /// Overwrite completion_ticks from an external source (e.g. runner sync).
    pub fn set_completion_ticks(&mut self, ticks: VecDeque<u64>) {
        self.completion_ticks = ticks;
    }

    pub fn record_completion(&mut self, tick: u64) {
        self.tasks_completed += 1;
        self.completion_ticks.push_back(tick);
        while self.completion_ticks.len() > THROUGHPUT_WINDOW_SIZE {
            self.completion_ticks.pop_front();
        }
        // Update O(1) throughput cache
        if tick == self.last_completion_tick {
            self.last_completion_count += 1;
        } else {
            self.last_completion_tick = tick;
            self.last_completion_count = 1;
        }
    }

    /// Number of tasks completed at the given tick (instantaneous count).
    /// O(1) for the most recent completion tick, falls back to deque scan
    /// for historical ticks.
    pub fn throughput(&self, current_tick: u64) -> f64 {
        if current_tick == self.last_completion_tick {
            self.last_completion_count as f64
        } else {
            // Historical query — scan the deque (rare: only in UI/export)
            self.completion_ticks.iter().filter(|&&t| t == current_tick).count() as f64
        }
    }
}

// Old recycle_goals / lifelong_replan ECS systems removed —
// SimulationRunner drives goal recycling and replanning internally.

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct TaskPlugin;

impl Plugin for TaskPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LifelongConfig>()
            .init_resource::<ActiveScheduler>()
            .init_resource::<DistanceMapCache>();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::topology::ZoneType;
    use rand::SeedableRng;
    use std::collections::HashMap;

    fn test_rng() -> ChaCha8Rng {
        ChaCha8Rng::seed_from_u64(42)
    }

    fn test_zones() -> ZoneMap {
        let mut zone_type = HashMap::new();
        let mut pickup_cells = Vec::new();
        let mut delivery_cells = Vec::new();
        let mut corridor_cells = Vec::new();

        // Small 8x8 zone map: bottom 2 rows = delivery, rest = pickup + corridor
        for x in 0..8 {
            for y in 0..8 {
                let pos = IVec2::new(x, y);
                if y < 2 {
                    zone_type.insert(pos, ZoneType::Delivery);
                    delivery_cells.push(pos);
                } else if y == 3 || y == 5 {
                    zone_type.insert(pos, ZoneType::Pickup);
                    pickup_cells.push(pos);
                } else {
                    zone_type.insert(pos, ZoneType::Corridor);
                    corridor_cells.push(pos);
                }
            }
        }

        ZoneMap {
            pickup_cells,
            delivery_cells,
            corridor_cells,
            recharging_cells: Vec::new(),
            zone_type,
            queue_lines: Vec::new(),
        }
    }

    #[test]
    fn random_scheduler_assigns_pickup() {
        let zones = test_zones();
        let scheduler = RandomScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 4);

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        let p = pickup.unwrap();
        assert!(zones.pickup_cells.contains(&p));
    }

    #[test]
    fn random_scheduler_assigns_delivery() {
        let zones = test_zones();
        let scheduler = RandomScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 4);

        let delivery = scheduler.assign_delivery(&zones, pos, &occupied, &mut rng);
        assert!(delivery.is_some());
        let d = delivery.unwrap();
        assert!(zones.delivery_cells.contains(&d));
    }

    #[test]
    fn closest_scheduler_picks_nearest_pickup() {
        let zones = test_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 4);

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        let p = pickup.unwrap();
        // Should be closest pickup cell to (4,4) — which is y=3 or y=5
        let dist = (p.x - pos.x).abs() + (p.y - pos.y).abs();
        assert!(dist <= 2, "closest pickup should be nearby, got dist={dist}");
    }

    #[test]
    fn closest_scheduler_assigns_random_delivery() {
        let zones = test_zones();
        let scheduler = ClosestFirstScheduler;
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 2);

        // Delivery assignment is now random (not nearest) to prevent
        // short-cycle inflation in compact maps. Verify it picks a valid
        // delivery cell and distributes across multiple cells over many seeds.
        let mut seen: HashSet<IVec2> = HashSet::new();
        for seed in 0..50u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let delivery = scheduler.assign_delivery(&zones, pos, &occupied, &mut rng);
            assert!(delivery.is_some());
            let d = delivery.unwrap();
            assert!(zones.delivery_cells.contains(&d), "must be a valid delivery cell");
            seen.insert(d);
        }
        assert!(
            seen.len() > 1,
            "delivery should distribute across multiple cells, got {}",
            seen.len()
        );
    }

    #[test]
    fn task_leg_default_is_idle() {
        let leg = TaskLeg::default();
        assert_eq!(leg, TaskLeg::Free);
    }

    #[test]
    fn task_leg_labels() {
        assert_eq!(TaskLeg::Free.label(), "free");
        assert_eq!(TaskLeg::TravelEmpty(IVec2::ZERO).label(), "travel_empty");
        assert_eq!(TaskLeg::Loading(IVec2::ZERO).label(), "loading");
        assert_eq!(
            TaskLeg::TravelToQueue { from: IVec2::ZERO, to: IVec2::ONE, line_index: 0 }.label(),
            "travel_to_queue"
        );
        assert_eq!(
            TaskLeg::Queuing { from: IVec2::ZERO, to: IVec2::ONE, line_index: 0 }.label(),
            "queuing"
        );
        assert_eq!(
            TaskLeg::TravelLoaded { from: IVec2::ZERO, to: IVec2::ONE }.label(),
            "travel_loaded"
        );
        assert_eq!(TaskLeg::Unloading { from: IVec2::ZERO, to: IVec2::ONE }.label(), "unloading");
        assert_eq!(TaskLeg::Charging.label(), "charging");
    }

    #[test]
    fn lifelong_config_throughput_instantaneous() {
        let mut config = LifelongConfig::default();

        // No completions → 0 at any tick
        assert_eq!(config.throughput(5), 0.0);

        // Single completion at tick 10 → 1 at tick 10, 0 elsewhere
        config.record_completion(10);
        assert_eq!(config.throughput(10), 1.0);
        assert_eq!(config.throughput(9), 0.0);
        assert_eq!(config.throughput(11), 0.0);

        // Two completions at tick 10 → 2 at tick 10
        config.record_completion(10);
        assert_eq!(config.throughput(10), 2.0);
        assert_eq!(config.throughput(11), 0.0);

        // One completion at tick 12 → 1 at tick 12, still 2 at tick 10
        config.record_completion(12);
        assert_eq!(config.throughput(12), 1.0);
        assert_eq!(config.throughput(10), 2.0);
    }

    #[test]
    fn lifelong_config_reset() {
        let mut config = LifelongConfig::default();
        config.enabled = true;
        config.record_completion(1);
        config.record_completion(2);
        config.needs_replan = true;

        config.reset();
        assert_eq!(config.tasks_completed, 0);
        assert!(!config.needs_replan);
        assert_eq!(config.throughput(1), 0.0);
        assert_eq!(config.throughput(2), 0.0);
    }

    #[test]
    fn active_scheduler_from_name() {
        let s = ActiveScheduler::from_name("random");
        assert_eq!(s.name(), "random");

        let s = ActiveScheduler::from_name("closest");
        assert_eq!(s.name(), "closest");

        // Unknown name falls back to random
        let s = ActiveScheduler::from_name("unknown");
        assert_eq!(s.name(), "random");
    }

    // ── assign_pickups_batch tests ────────────────────────────────────────────

    /// Regression: closest scheduler must NOT always assign the globally-nearest
    /// pickup cell. When multiple agents share a delivery zone (e.g. all at x=30),
    /// they should receive different pickup cells drawn from a random pool.
    #[test]
    fn closest_batch_no_hotspot_regression() {
        // Simulate warehouse_large geometry: pickup rows at y=1,4,7 spanning x=2..8,
        // delivery column at x=10. After delivery all agents are near x=10.
        let mut zone_type = std::collections::HashMap::new();
        let mut pickup_cells = Vec::new();
        let delivery_cells: Vec<IVec2> = (1..8).map(|y| IVec2::new(10, y)).collect();
        for y in [1, 4, 7] {
            for x in 2..9 {
                let pos = IVec2::new(x, y);
                zone_type.insert(pos, crate::core::topology::ZoneType::Pickup);
                pickup_cells.push(pos);
            }
        }
        for &c in &delivery_cells {
            zone_type.insert(c, crate::core::topology::ZoneType::Delivery);
        }
        let zones = ZoneMap {
            pickup_cells,
            delivery_cells,
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type,
            queue_lines: Vec::new(),
        };

        // 5 agents all clustered near x=10 (post-delivery position)
        let free_agents: Vec<(usize, IVec2)> =
            (0..5).map(|i| (i, IVec2::new(10, i as i32 + 1))).collect();

        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();
        let mut used_goals = HashSet::new();

        let assignments =
            scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);

        // All 5 agents must be assigned
        assert_eq!(assignments.len(), 5, "all free agents should be assigned");

        // All assigned pickups must be valid pickup cells
        for (_, pickup) in &assignments {
            assert!(zones.pickup_cells.contains(pickup), "assigned cell must be a pickup cell");
        }

        // No two agents should be assigned the same pickup cell
        let unique: HashSet<IVec2> = assignments.iter().map(|(_, p)| *p).collect();
        assert_eq!(unique.len(), 5, "all pickups must be distinct");

        // Key regression: assigned cells must NOT all be in the same x column.
        // If hotspot bug were present, all would get x=8 (the globally nearest column).
        let xs: HashSet<i32> = assignments.iter().map(|(_, p)| p.x).collect();
        assert!(
            xs.len() > 1,
            "closest batch must distribute across multiple x columns, got: {xs:?}"
        );
    }

    /// The random pool in closest-batch must cover all pickup rows, not just
    /// the nearest row to the delivery zone.
    #[test]
    fn closest_batch_uses_random_candidates() {
        let zones = test_zones(); // pickup rows at y=3 and y=5
        let scheduler = ClosestFirstScheduler;
        let mut used_goals = HashSet::new();

        // Run many times — with random candidates, both y=3 and y=5 rows must appear.
        let mut saw_y3 = false;
        let mut saw_y5 = false;
        for seed in 0..50u64 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            // Single agent at y=4 — globally closest is y=3 or y=5 (equal distance).
            // Without random candidates the same row would always win; with random
            // candidates both rows should appear across many seeds.
            let free_agents = vec![(0usize, IVec2::new(4, 4))];
            let result =
                scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);
            if let Some((_, p)) = result.first() {
                if p.y == 3 {
                    saw_y3 = true;
                }
                if p.y == 5 {
                    saw_y5 = true;
                }
            }
        }
        assert!(saw_y3, "random candidates should sometimes select from y=3 row");
        assert!(saw_y5, "random candidates should sometimes select from y=5 row");
    }

    /// Default batch behaviour (random): no regression.
    #[test]
    fn random_batch_assigns_all_free_agents() {
        let zones = test_zones();
        let scheduler = RandomScheduler;
        let mut rng = test_rng();
        let mut used_goals = HashSet::new();

        let free_agents: Vec<(usize, IVec2)> =
            vec![(0, IVec2::new(0, 7)), (1, IVec2::new(4, 7)), (2, IVec2::new(7, 7))];
        let assignments =
            scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);
        assert_eq!(assignments.len(), 3);
        for (_, p) in &assignments {
            assert!(zones.pickup_cells.contains(p));
        }
        // No duplicates
        let unique: HashSet<IVec2> = assignments.iter().map(|(_, p)| *p).collect();
        assert_eq!(unique.len(), 3);
    }

    /// `recycle_goals_core` with closest scheduler must not emit duplicate pickup goals.
    #[test]
    fn recycle_goals_core_closest_no_duplicate_pickups() {
        let zones = test_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();

        // 4 agents all Free, all at the same position (extreme hotspot scenario)
        let agents: Vec<TaskAgentSnapshot> = (0..4)
            .map(|_| TaskAgentSnapshot {
                pos: IVec2::new(4, 7), // near pickup rows at y=3 and y=5
                goal: IVec2::new(4, 7),
                task_leg: TaskLeg::Free,
                alive: true,
                frozen: false,
            })
            .collect();

        let result = recycle_goals_core(&agents, &scheduler, &zones, &mut rng, 1);

        // All 4 agents should be assigned (zones.pickup_cells has 16 cells)
        let assigned: Vec<_> = result
            .updates
            .iter()
            .filter(|u| matches!(u.task_leg, TaskLeg::TravelEmpty(_)))
            .collect();
        assert_eq!(assigned.len(), 4, "all 4 agents must be assigned pickup goals");

        // No two agents should share the same goal
        let goals: HashSet<IVec2> = assigned.iter().map(|u| u.goal).collect();
        assert_eq!(goals.len(), 4, "all pickup goals must be distinct");
    }

    // ── D1: Delivery fairness ───────────────────────────────────────────

    #[test]
    fn delivery_nearest_distributes() {
        // 7 delivery cells at x=10, y=1..7
        let mut zone_type = std::collections::HashMap::new();
        let delivery_cells: Vec<IVec2> = (1..=7).map(|y| IVec2::new(10, y)).collect();
        let pickup_cells: Vec<IVec2> = vec![IVec2::new(2, 4)]; // minimal
        for &c in &delivery_cells {
            zone_type.insert(c, crate::core::topology::ZoneType::Delivery);
        }
        let zones = ZoneMap {
            pickup_cells,
            delivery_cells: delivery_cells.clone(),
            corridor_cells: Vec::new(),
            recharging_cells: Vec::new(),
            zone_type,
            queue_lines: Vec::new(),
        };

        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();
        let mut counts: std::collections::HashMap<IVec2, usize> = std::collections::HashMap::new();

        // 100 agents at various y positions
        for i in 0..100 {
            let pos = IVec2::new(5, (i % 7) as i32 + 1); // distribute across y=1..7
            let mut occupied = HashSet::new();
            // Add some already-occupied cells to force variety
            if i % 3 == 0 {
                occupied.insert(IVec2::new(10, (i % 7) as i32 + 1));
            }
            if let Some(cell) = scheduler.assign_delivery(&zones, pos, &occupied, &mut rng) {
                *counts.entry(cell).or_insert(0) += 1;
            }
        }

        let total: usize = counts.values().sum();
        let max_count = counts.values().copied().max().unwrap_or(0);
        let max_ratio = max_count as f64 / total as f64;

        // At least 3 different delivery cells should be used
        assert!(counts.len() >= 3, "only {} delivery cells used out of 7", counts.len());
        assert!(max_ratio < 0.40, "single cell hotspot: {:.1}% of deliveries", max_ratio * 100.0);
    }

    // ── D2: Queue policy distribution ───────────────────────────────────

    #[test]
    fn queue_policy_distributes() {
        use crate::core::action::Direction;
        use crate::core::queue::{
            ClosestQueuePolicy, DeliveryQueuePolicy, QueueDecision, QueueLine, QueueState,
        };

        // Create 4 queue lines manually (no grid needed — we build them directly)
        let queue_lines: Vec<QueueLine> = (0..4)
            .map(|i| QueueLine {
                delivery_cell: IVec2::new(11, i * 3 + 2),
                direction: Direction::South,
                cells: vec![
                    IVec2::new(10, i * 3 + 1),
                    IVec2::new(10, i * 3 + 2),
                    IVec2::new(10, i * 3 + 3),
                ],
            })
            .collect();

        // Use ClosestQueuePolicy to assign 20 agents
        let policy = ClosestQueuePolicy;
        let mut states: Vec<QueueState> = queue_lines
            .iter()
            .enumerate()
            .map(|(i, q)| QueueState::new(i, q.cells.len()))
            .collect();
        let mut used_queues: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for i in 0..20 {
            let pos = IVec2::new(5, (i % 12) as i32 + 1); // various positions
            let decision = policy.choose_queue(pos, &queue_lines, &states);
            if let QueueDecision::JoinQueue { line_index } = decision {
                // Mark a slot as occupied to simulate filling
                if let Some(slot) = states[line_index].slots.iter_mut().find(|s| s.is_none()) {
                    *slot = Some(i);
                }
                used_queues.insert(line_index);
            }
        }

        assert!(used_queues.len() >= 2, "only {} queue lines used out of 4", used_queues.len());
    }

    // ── Frozen agents don't block scheduling ───────────────────────────

    #[test]
    fn frozen_agents_goals_do_not_block_pickup_assignment() {
        let zones = test_zones(); // 16 pickup cells
        let scheduler = RandomScheduler;
        let mut rng = test_rng();

        // 12 frozen agents each heading to a unique pickup cell.
        // Without the fix, these would block 12 of 16 cells, leaving only 4.
        let frozen_goals: Vec<IVec2> = zones.pickup_cells[..12].to_vec();
        let mut agents: Vec<TaskAgentSnapshot> = frozen_goals
            .iter()
            .map(|&goal| TaskAgentSnapshot {
                pos: IVec2::new(0, 0), // far from goal → pos != goal → would enter used_goals
                goal,
                task_leg: TaskLeg::TravelEmpty(goal),
                alive: true,
                frozen: true, // latency-injected
            })
            .collect();

        // 2 active Free agents ready for pickup assignment
        for _ in 0..2 {
            agents.push(TaskAgentSnapshot {
                pos: IVec2::new(4, 1),
                goal: IVec2::new(4, 1),
                task_leg: TaskLeg::Free,
                alive: true,
                frozen: false,
            });
        }

        // Run 50 rounds and collect unique pickup cells assigned
        let mut all_pickups: HashSet<IVec2> = HashSet::new();
        for tick in 1..=50 {
            let result = recycle_goals_core(&agents, &scheduler, &zones, &mut rng, tick);
            for update in &result.updates {
                if let TaskLeg::TravelEmpty(cell) = &update.task_leg {
                    all_pickups.insert(*cell);
                }
            }
        }

        // With frozen agents excluded from used_goals, all 16 pickup cells
        // should be reachable. Without the fix, only 4 would be available.
        assert!(
            all_pickups.len() > 4,
            "frozen agents' goals should not block scheduling: only {} unique cells assigned",
            all_pickups.len()
        );
    }
}
