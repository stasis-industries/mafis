use bevy::prelude::*;
use rand::Rng;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashMap, HashSet, VecDeque};

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
    /// `RandomScheduler` and `BalancedScheduler` which already pick diversely).
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
// Task creation helper
// ---------------------------------------------------------------------------

/// Select one cell uniformly at random from `cells` that is not in `occupied`.
///
/// Used for **task creation** (generating a new task regardless of which agent
/// will be assigned to it). Unlike per-agent selection, there is no agent-position
/// bias — any available cell is equally valid.
fn random_cell_from(
    cells: &[IVec2],
    occupied: &HashSet<IVec2>,
    rng: &mut ChaCha8Rng,
) -> Option<IVec2> {
    if cells.is_empty() {
        return None;
    }
    // Rejection sampling — fast in the common case (most cells free)
    for _ in 0..200 {
        let idx = rng.random_range(0..cells.len());
        let cell = cells[idx];
        if !occupied.contains(&cell) {
            return Some(cell);
        }
    }
    // Fallback: collect all available cells and pick uniformly
    let valid: Vec<IVec2> = cells.iter().copied().filter(|c| !occupied.contains(c)).collect();
    if valid.is_empty() {
        None
    } else {
        Some(valid[rng.random_range(0..valid.len())])
    }
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

    fn assign_pickups_batch(
        &self,
        free_agents: &[(usize, IVec2)],
        zones: &ZoneMap,
        used_goals: &mut HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Vec<(usize, IVec2)> {
        if free_agents.is_empty() {
            return Vec::new();
        }

        // Phase 1 — task creation: randomly generate one candidate pickup per free agent.
        // Creation is uniform-random and position-independent — no agent-position bias.
        let mut pool_used = used_goals.clone();
        let mut task_pool: Vec<IVec2> = Vec::with_capacity(free_agents.len());
        for _ in 0..free_agents.len() {
            if let Some(cell) = random_cell_from(&zones.pickup_cells, &pool_used, rng) {
                pool_used.insert(cell);
                task_pool.push(cell);
            } else {
                break; // No more available pickup cells
            }
        }
        if task_pool.is_empty() {
            return Vec::new();
        }

        // Phase 2 — task assignment: each agent picks the nearest task in the
        // random pool. This avoids convergence on a fixed hotspot because the
        // pool is random; agents still prefer shorter trips.
        let mut available = task_pool;
        let mut assignments = Vec::with_capacity(available.len());
        for &(idx, pos) in free_agents {
            if available.is_empty() {
                break;
            }
            let best = available
                .iter()
                .enumerate()
                .min_by_key(|&(_, c)| (c.x - pos.x).abs() + (c.y - pos.y).abs());
            if let Some((ti, &pickup)) = best {
                available.swap_remove(ti);
                used_goals.insert(pickup);
                assignments.push((idx, pickup));
            }
        }
        assignments
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
///
/// # Task assignment model
///
/// Task creation (which pickup cells exist) is always random. The scheduler
/// controls only task *assignment* (which agent goes to which task). All Free
/// agents are batch-assigned via `TaskScheduler::assign_pickups_batch` so that
/// schedulers like `ClosestFirst` can implement proper random-create/closest-assign
/// semantics instead of converging on a positional hotspot.
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

    // ── Pass 1: batch-assign pickups to all Free agents ──────────────────────
    // Collect every alive Free agent that has reached its goal this tick.
    let free_agents: Vec<(usize, IVec2)> = agents
        .iter()
        .enumerate()
        .filter(|(_, a)| a.alive && a.pos == a.goal && matches!(a.task_leg, TaskLeg::Free))
        .map(|(i, a)| (i, a.pos))
        .collect();

    // The scheduler generates random task candidates and assigns them.
    // used_goals is updated in-place so subsequent passes see occupied cells.
    let pickup_assignments: HashMap<usize, IVec2> = scheduler
        .assign_pickups_batch(&free_agents, zones, &mut used_goals, rng)
        .into_iter()
        .collect();

    if !pickup_assignments.is_empty() {
        needs_replan = true;
    }

    // ── Pass 2: apply assignments and process all other state transitions ─────
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
                if let Some(&pickup) = pickup_assignments.get(&i) {
                    updates[i].task_leg = TaskLeg::TravelEmpty(pickup);
                    updates[i].goal = pickup;
                    updates[i].path_cleared = true;
                }
                // If no assignment (all pickups claimed), agent stays Free.
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
                // Delivery complete → transition to Free. Do NOT immediately
                // assign a new pickup — that would make Free a 0-tick state.
                // The next tick's Free→TravelEmpty handles reassignment.
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

    // ── assign_pickups_batch tests ────────────────────────────────────────────

    /// Regression: closest scheduler must NOT always assign the globally-nearest
    /// pickup cell. When multiple agents share a delivery zone (e.g. all at x=30),
    /// they should receive different pickup cells drawn from a random pool.
    #[test]
    fn closest_batch_no_hotspot_regression() {
        // Simulate warehouse_medium geometry: pickup rows at y=1,4,7 spanning x=2..8,
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
        let free_agents: Vec<(usize, IVec2)> = (0..5)
            .map(|i| (i, IVec2::new(10, i as i32 + 1)))
            .collect();

        let scheduler = ClosestFirstScheduler;
        let mut rng = test_rng();
        let mut used_goals = HashSet::new();

        let assignments = scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);

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
        assert!(xs.len() > 1, "closest batch must distribute across multiple x columns, got: {xs:?}");
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
            let result = scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);
            if let Some((_, p)) = result.first() {
                if p.y == 3 { saw_y3 = true; }
                if p.y == 5 { saw_y5 = true; }
            }
        }
        assert!(saw_y3, "random candidates should sometimes select from y=3 row");
        assert!(saw_y5, "random candidates should sometimes select from y=5 row");
    }

    /// Default batch behaviour (random / balanced): no regression.
    #[test]
    fn random_batch_assigns_all_free_agents() {
        let zones = test_zones();
        let scheduler = RandomScheduler;
        let mut rng = test_rng();
        let mut used_goals = HashSet::new();

        let free_agents: Vec<(usize, IVec2)> = vec![
            (0, IVec2::new(0, 7)),
            (1, IVec2::new(4, 7)),
            (2, IVec2::new(7, 7)),
        ];
        let assignments = scheduler.assign_pickups_batch(&free_agents, &zones, &mut used_goals, &mut rng);
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
            })
            .collect();

        let result = recycle_goals_core(&agents, &scheduler, &zones, &mut rng, 1);

        // All 4 agents should be assigned (zones.pickup_cells has 16 cells)
        let assigned: Vec<_> = result.updates.iter()
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
        use crate::core::queue::{QueueLine, QueueState, ClosestQueuePolicy, DeliveryQueuePolicy, QueueDecision};
        use crate::core::action::Direction;

        // Create 4 queue lines manually (no grid needed — we build them directly)
        let queue_lines: Vec<QueueLine> = (0..4).map(|i| {
            QueueLine {
                delivery_cell: IVec2::new(11, i * 3 + 2),
                direction: Direction::South,
                cells: vec![
                    IVec2::new(10, i * 3 + 1),
                    IVec2::new(10, i * 3 + 2),
                    IVec2::new(10, i * 3 + 3),
                ],
            }
        }).collect();

        // Use ClosestQueuePolicy to assign 20 agents
        let policy = ClosestQueuePolicy;
        let mut states: Vec<QueueState> = queue_lines.iter().enumerate()
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
}
