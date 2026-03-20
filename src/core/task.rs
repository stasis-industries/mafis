use bevy::prelude::*;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashSet, VecDeque};

use super::topology::ZoneMap;
use crate::constants::THROUGHPUT_WINDOW_SIZE;
use crate::solver::heuristics::DistanceMapCache;

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
    /// At delivery zone, unloading cargo (0-tick dwell for now).
    Unloading { from: IVec2, to: IVec2 },
    /// Placeholder — triggered by future energy system.
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
}

// ---------------------------------------------------------------------------
// RandomScheduler
// ---------------------------------------------------------------------------

pub struct RandomScheduler;

impl RandomScheduler {
    fn random_from_cells(
        cells: &[IVec2],
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        if cells.is_empty() {
            return None;
        }
        // Try random sampling first
        for _ in 0..200 {
            let idx = rng.random_range(0..cells.len());
            let cell = cells[idx];
            if cell != pos && !occupied.contains(&cell) {
                return Some(cell);
            }
        }
        // Fallback: linear scan
        let valid: Vec<IVec2> = cells
            .iter()
            .copied()
            .filter(|&c| c != pos && !occupied.contains(&c))
            .collect();
        if valid.is_empty() {
            // All cells claimed — caller should keep agent waiting
            None
        } else {
            Some(valid[rng.random_range(0..valid.len())])
        }
    }
}

impl TaskScheduler for RandomScheduler {
    fn name(&self) -> &str {
        "random"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::random_from_cells(&zones.pickup_cells, pos, occupied, rng)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::random_from_cells(&zones.delivery_cells, pos, occupied, rng)
    }
}

// ---------------------------------------------------------------------------
// ClosestFirstScheduler
// ---------------------------------------------------------------------------

pub struct ClosestFirstScheduler;

impl ClosestFirstScheduler {
    fn nearest_from_cells(
        cells: &[IVec2],
        pos: IVec2,
        occupied: &HashSet<IVec2>,
    ) -> Option<IVec2> {
        if cells.is_empty() {
            return None;
        }
        // Prefer closest unoccupied cell
        let best = cells
            .iter()
            .copied()
            .filter(|&c| c != pos && !occupied.contains(&c))
            .min_by_key(|c| (c.x - pos.x).abs() + (c.y - pos.y).abs());
        if best.is_some() {
            return best;
        }
        // All cells claimed — caller should keep agent waiting
        None
    }
}

impl TaskScheduler for ClosestFirstScheduler {
    fn name(&self) -> &str {
        "closest"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        _rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::nearest_from_cells(&zones.pickup_cells, pos, occupied)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        _rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::nearest_from_cells(&zones.delivery_cells, pos, occupied)
    }
}

// ---------------------------------------------------------------------------
// BalancedScheduler
// ---------------------------------------------------------------------------

/// Assigns tasks to the least-recently-used cell, tie-breaking by distance.
/// Distributes load evenly across all pickup/delivery cells, reducing hotspots.
pub struct BalancedScheduler;

impl BalancedScheduler {
    fn least_used_cell(
        cells: &[IVec2],
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        if cells.is_empty() {
            return None;
        }
        // Count how many agents are currently targeting each cell (proxy for usage).
        // occupied = cells that are already claimed as goals.
        let free: Vec<IVec2> = cells
            .iter()
            .copied()
            .filter(|&c| c != pos && !occupied.contains(&c))
            .collect();
        if free.is_empty() {
            return None;
        }
        // Pick the farthest free cell from centroid of occupied cells — spreads agents out.
        // When occupied is empty, fall back to random.
        if occupied.is_empty() {
            return Some(free[rng.random_range(0..free.len())]);
        }
        let n = occupied.len() as f32;
        let cx = occupied.iter().map(|c| c.x as f32).sum::<f32>() / n;
        let cy = occupied.iter().map(|c| c.y as f32).sum::<f32>() / n;
        // Sort by distance from centroid (descending) to pick the most spread-out cell.
        // Tie-break: closest to agent (minimize travel).
        let best = free.iter().copied().max_by(|a, b| {
            let da = (a.x as f32 - cx).abs() + (a.y as f32 - cy).abs();
            let db = (b.x as f32 - cx).abs() + (b.y as f32 - cy).abs();
            da.partial_cmp(&db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    // Tie-break: prefer closer to agent
                    let dist_a = (a.x - pos.x).abs() + (a.y - pos.y).abs();
                    let dist_b = (b.x - pos.x).abs() + (b.y - pos.y).abs();
                    dist_b.cmp(&dist_a) // reverse: smaller distance = better
                })
        });
        best
    }
}

impl TaskScheduler for BalancedScheduler {
    fn name(&self) -> &str {
        "balanced"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::least_used_cell(&zones.pickup_cells, pos, occupied, rng)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        Self::least_used_cell(&zones.delivery_cells, pos, occupied, rng)
    }
}

// ---------------------------------------------------------------------------
// RoundTripScheduler (warehouse-aware)
// ---------------------------------------------------------------------------

/// Warehouse-aware scheduler that minimizes total round-trip distance.
/// Pickup: chooses the cell that minimizes `dist(agent→pickup) + min(dist(pickup→any_delivery))`.
/// Delivery: chooses nearest delivery cell (same as closest).
pub struct RoundTripScheduler;

impl RoundTripScheduler {
    fn min_delivery_dist(pickup: IVec2, delivery_cells: &[IVec2]) -> i32 {
        delivery_cells
            .iter()
            .map(|d| (d.x - pickup.x).abs() + (d.y - pickup.y).abs())
            .min()
            .unwrap_or(0)
    }
}

impl TaskScheduler for RoundTripScheduler {
    fn name(&self) -> &str {
        "roundtrip"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        _rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        if zones.pickup_cells.is_empty() {
            return None;
        }
        zones
            .pickup_cells
            .iter()
            .copied()
            .filter(|&c| c != pos && !occupied.contains(&c))
            .min_by_key(|&pickup| {
                let to_pickup = (pickup.x - pos.x).abs() + (pickup.y - pos.y).abs();
                let pickup_to_delivery = Self::min_delivery_dist(pickup, &zones.delivery_cells);
                to_pickup + pickup_to_delivery
            })
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        _rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        ClosestFirstScheduler::nearest_from_cells(&zones.delivery_cells, pos, occupied)
    }
}

// ---------------------------------------------------------------------------
// ActiveScheduler resource
// ---------------------------------------------------------------------------

pub const SCHEDULER_NAMES: &[(&str, &str)] = &[
    ("random", "Random"),
    ("closest", "Closest"),
    ("balanced", "Balanced"),
    ("roundtrip", "Round-Trip"),
];

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
            "balanced" => Box::new(BalancedScheduler),
            "roundtrip" => Box::new(RoundTripScheduler),
            _ => Box::new(RandomScheduler),
        };
        let actual_name = scheduler.name().to_string();
        Self {
            scheduler,
            name: actual_name,
        }
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
            self.last_completion_count = completion_ticks.iter().rev().take_while(|&&t| t == last_tick).count() as u64;
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

// ---------------------------------------------------------------------------
// Core recycle_goals (shared by ECS and headless baseline)
// ---------------------------------------------------------------------------

/// Agent snapshot for the recycle_goals core function.
pub struct TaskAgentSnapshot {
    pub pos: IVec2,
    pub goal: IVec2,
    pub task_leg: TaskLeg,
    pub alive: bool,
}

/// Per-agent update from recycle_goals_core.
pub struct TaskUpdate {
    pub task_leg: TaskLeg,
    pub goal: IVec2,
    pub path_cleared: bool,
}

/// Aggregate result from recycle_goals_core.
pub struct RecycleResult {
    pub updates: Vec<TaskUpdate>,
    pub completion_ticks: Vec<u64>,
    pub needs_replan: bool,
    /// Agent indices that just entered Loading this tick (must NOT be
    /// processed by the queue manager on the same tick — ensures Loading
    /// is visible for at least 1 tick).
    pub just_loaded: Vec<usize>,
}

/// Pure task recycling logic: checks agents at goal, assigns new tasks.
///
/// Both the live ECS system and headless baseline call this.
/// Agents MUST be pre-sorted by index before calling.
///
/// State transitions enforce a 1-tick minimum dwell:
/// - `TravelLoaded` → `Free` (not immediately `TravelEmpty`)
/// - `TravelEmpty` → `Loading` (queue manager must wait 1 tick via `just_loaded`)
///
/// This ensures the user can observe each state in the UI.
pub fn recycle_goals_core(
    agents: &[TaskAgentSnapshot],
    scheduler: &dyn TaskScheduler,
    zones: &ZoneMap,
    rng: &mut ChaCha8Rng,
    tick: u64,
) -> RecycleResult {
    let mut used_goals: HashSet<IVec2> = agents
        .iter()
        .filter(|a| a.alive && a.pos != a.goal)
        .map(|a| a.goal)
        .collect();

    let mut updates: Vec<TaskUpdate> = agents
        .iter()
        .map(|a| TaskUpdate {
            task_leg: a.task_leg.clone(),
            goal: a.goal,
            path_cleared: false,
        })
        .collect();

    let mut completion_ticks = Vec::new();
    let mut needs_replan = false;
    let mut just_loaded = Vec::new();

    for (i, agent) in agents.iter().enumerate() {
        // Skip dead agents — they must not consume scheduler assignments
        if !agent.alive {
            continue;
        }

        if agent.pos != agent.goal {
            continue;
        }

        match &agent.task_leg {
            TaskLeg::Free => {
                if let Some(pickup) =
                    scheduler.assign_pickup(zones, agent.pos, &used_goals, rng)
                {
                    updates[i].task_leg = TaskLeg::TravelEmpty(pickup);
                    updates[i].goal = pickup;
                    updates[i].path_cleared = true;
                    used_goals.insert(pickup);
                    needs_replan = true;
                }
            }
            TaskLeg::TravelEmpty(pickup_cell) => {
                // Transition to Loading — queue manager handles delivery assignment
                // on the NEXT tick (via just_loaded skip set).
                let pickup = *pickup_cell;
                updates[i].task_leg = TaskLeg::Loading(pickup);
                just_loaded.push(i);
            }
            TaskLeg::Loading(_) => {
                // Loading agents wait for queue manager to assign a delivery queue.
                // No action here — QueueManager::tick() processes Loading → TravelToQueue.
            }
            TaskLeg::TravelToQueue { .. } => {
                // TravelToQueue agents are heading to the back of a queue line.
                // Managed by QueueManager (arrivals → Queuing).
            }
            TaskLeg::Queuing { .. } => {
                // Queuing agents are physically in a queue slot (compact, promote).
                // Managed by QueueManager.
            }
            TaskLeg::TravelLoaded { .. } => {
                // Delivery complete → transition to Idle. Do NOT immediately
                // assign a new pickup — that would make Idle a 0-tick state.
                // The next tick's Idle→TravelToLoad handles reassignment.
                updates[i].task_leg = TaskLeg::Free;
                completion_ticks.push(tick);
                needs_replan = true;
            }
            TaskLeg::Unloading { .. } => {}
            TaskLeg::Charging => {}
        }
    }

    RecycleResult {
        updates,
        completion_ticks,
        needs_replan,
        just_loaded,
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

        ZoneMap { pickup_cells, delivery_cells, corridor_cells, recharging_cells: Vec::new(), zone_type, queue_lines: Vec::new() }
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
    fn closest_scheduler_picks_nearest_delivery() {
        let zones = test_zones();
        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 2);

        let delivery = scheduler.assign_delivery(&zones, pos, &occupied, &mut rng);
        assert!(delivery.is_some());
        let d = delivery.unwrap();
        let dist = (d.x - pos.x).abs() + (d.y - pos.y).abs();
        assert!(dist <= 2, "closest delivery should be nearby, got dist={dist}");
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
        assert_eq!(TaskLeg::TravelToQueue { from: IVec2::ZERO, to: IVec2::ONE, line_index: 0 }.label(), "travel_to_queue");
        assert_eq!(TaskLeg::Queuing { from: IVec2::ZERO, to: IVec2::ONE, line_index: 0 }.label(), "queuing");
        assert_eq!(TaskLeg::TravelLoaded { from: IVec2::ZERO, to: IVec2::ONE }.label(), "travel_loaded");
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
    fn balanced_scheduler_assigns_pickup() {
        let zones = test_zones();
        let scheduler = BalancedScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 4);

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        assert!(zones.pickup_cells.contains(&pickup.unwrap()));
    }

    #[test]
    fn balanced_scheduler_spreads_assignments() {
        let zones = test_zones();
        let scheduler = BalancedScheduler;
        let mut rng = test_rng();
        let pos = IVec2::new(4, 4);

        // Claim some cells as occupied
        let mut occupied = HashSet::new();
        occupied.insert(IVec2::new(4, 3)); // one pickup cell occupied

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        let p = pickup.unwrap();
        // Should not assign the occupied cell
        assert!(!occupied.contains(&p));
    }

    #[test]
    fn roundtrip_scheduler_assigns_pickup() {
        let zones = test_zones();
        let scheduler = RoundTripScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        let pos = IVec2::new(4, 4);

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        assert!(zones.pickup_cells.contains(&pickup.unwrap()));
    }

    #[test]
    fn roundtrip_scheduler_prefers_close_to_delivery() {
        let zones = test_zones();
        let scheduler = RoundTripScheduler;
        let mut rng = test_rng();
        let occupied = HashSet::new();
        // Agent at top of map — should prefer pickup cells closer to delivery zone (bottom)
        let pos = IVec2::new(4, 7);

        let pickup = scheduler.assign_pickup(&zones, pos, &occupied, &mut rng);
        assert!(pickup.is_some());
        let p = pickup.unwrap();
        // y=3 pickup cells are closer to delivery (y<2) than y=5 cells
        assert_eq!(p.y, 3, "roundtrip should prefer pickup closer to delivery zone");
    }

    #[test]
    fn active_scheduler_from_name() {
        let s = ActiveScheduler::from_name("random");
        assert_eq!(s.name(), "random");

        let s = ActiveScheduler::from_name("closest");
        assert_eq!(s.name(), "closest");

        let s = ActiveScheduler::from_name("balanced");
        assert_eq!(s.name(), "balanced");

        let s = ActiveScheduler::from_name("roundtrip");
        assert_eq!(s.name(), "roundtrip");

        // Unknown name falls back to random
        let s = ActiveScheduler::from_name("unknown");
        assert_eq!(s.name(), "random");
    }
}
