use bevy::prelude::*;
use std::collections::HashMap;

use super::action::Direction;
use super::grid::GridMap;
use super::queue::{build_queue_lines, QueueLine};

// ---------------------------------------------------------------------------
// ZoneType + ZoneMap
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    Storage,     // obstacle (shelf) — not walkable
    Pickup,      // walkable, adjacent to storage row
    Delivery,    // delivery zone
    Corridor,    // main horizontal aisle
    CrossAisle,  // vertical aisle cutting through storage rows
    Open,        // generic walkable (open floor topology)
    Recharging,  // recharging station — walkable
}

#[derive(Resource, Debug, Clone)]
#[derive(Default)]
pub struct ZoneMap {
    pub pickup_cells: Vec<IVec2>,
    pub delivery_cells: Vec<IVec2>,
    pub corridor_cells: Vec<IVec2>,
    pub recharging_cells: Vec<IVec2>,
    pub zone_type: HashMap<IVec2, ZoneType>,
    /// Directed queue lines for delivery zones. Each delivery cell with a
    /// `queue_direction` gets a queue line extending in that direction.
    pub queue_lines: Vec<QueueLine>,
}


// ---------------------------------------------------------------------------
// Topology trait
// ---------------------------------------------------------------------------

pub struct TopologyOutput {
    pub grid: GridMap,
    pub zones: ZoneMap,
    pub suggested_agents: usize,
}

pub trait Topology: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn generate(&self, seed: u64) -> TopologyOutput;
}

// ---------------------------------------------------------------------------
// CustomMap — wraps raw grid + zones from the map maker
// ---------------------------------------------------------------------------

pub struct CustomMap {
    pub grid: GridMap,
    pub zones: ZoneMap,
}

impl Topology for CustomMap {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn generate(&self, _seed: u64) -> TopologyOutput {
        TopologyOutput {
            grid: GridMap::with_obstacles(
                self.grid.width,
                self.grid.height,
                self.grid.obstacles().clone(),
            ),
            zones: self.zones.clone(),
            suggested_agents: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// ActiveTopology resource
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct ActiveTopology {
    topology: Box<dyn Topology>,
    name: String,
}

impl ActiveTopology {
    /// Build an `ActiveTopology` from a registry entry's JSON data.
    /// Returns `None` if the JSON cannot be parsed.
    pub fn from_entry(entry: &TopologyEntry) -> Option<Self> {
        let (grid, zones) = TopologyRegistry::parse_entry(entry)?;
        let name = entry.id.clone();
        Some(Self {
            topology: Box::new(CustomMap { grid, zones }),
            name,
        })
    }

    /// Look up a topology by name from a pre-loaded registry.
    ///
    /// Falls back to the first available topology if `name` is not found.
    /// Panics if the registry is empty — this is a configuration error.
    pub fn from_registry(name: &str, registry: &TopologyRegistry) -> Self {
        if let Some(entry) = registry.find(name) {
            if let Some(at) = Self::from_entry(entry) {
                return at;
            }
        }
        // Fallback: first available topology
        if let Some(entry) = registry.entries.first() {
            if let Some(at) = Self::from_entry(entry) {
                return at;
            }
        }
        panic!(
            "No topologies found in registry. \
             Add at least one JSON topology file and run `sh topologies/build-manifest.sh`."
        );
    }

    /// Load a topology by name from the `topologies/` directory.
    ///
    /// On native, reads the registry from disk. On WASM, panics — use
    /// `from_registry` or `from_entry` instead (the bridge sets topology
    /// via the registry resource before any simulation starts).
    pub fn from_name(name: &str) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
            Self::from_registry(name, &registry)
        }
        #[cfg(target_arch = "wasm32")]
        {
            panic!(
                "ActiveTopology::from_name(\"{}\") called on WASM. \
                 Use from_registry() or from_entry() with the TopologyRegistry resource.",
                name,
            );
        }
    }

    pub fn topology(&self) -> &dyn Topology {
        self.topology.as_ref()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set(&mut self, topology: Box<dyn Topology>) {
        self.name = topology.name().to_string();
        self.topology = topology;
    }
}

impl Default for ActiveTopology {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
            if let Some(entry) = registry.entries.first() {
                if let Some(at) = Self::from_entry(entry) {
                    return at;
                }
            }
        }
        // WASM or empty registry: create a minimal 10×10 open grid
        let grid = GridMap::new(10, 10);
        let zones = ZoneMap::default();
        Self {
            topology: Box::new(CustomMap { grid, zones }),
            name: "empty".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// TopologyRegistry — centralized topology list from web/topologies/ JSON files
// ---------------------------------------------------------------------------

/// Metadata for a topology loaded from a JSON file.
#[derive(Debug, Clone)]
pub struct TopologyEntry {
    pub id: String,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub suggested_agents: usize,
    pub walkable_cells: usize,
    /// Raw JSON string for passing to the experiment runner or custom map loader.
    pub json_data: String,
}

/// Registry of all available topologies, populated from `web/topologies/` JSON files.
#[derive(Resource, Debug, Clone, Default)]
pub struct TopologyRegistry {
    pub entries: Vec<TopologyEntry>,
}

impl TopologyRegistry {
    /// Find an entry by id (e.g. "warehouse_medium").
    pub fn find(&self, id: &str) -> Option<&TopologyEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Load topology JSON files from a directory by reading its manifest.json.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_from_dir(dir: &std::path::Path) -> Self {
        let mut entries = Vec::new();
        let manifest_path = dir.join("manifest.json");

        let manifest_str = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(_) => return Self { entries },
        };

        let filenames: Vec<String> = match serde_json::from_str(&manifest_str) {
            Ok(v) => v,
            Err(_) => return Self { entries },
        };

        for filename in filenames {
            let path = dir.join(&filename);
            let json_data = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let v: serde_json::Value = match serde_json::from_str(&json_data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let width = v.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let height = v.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let name = v.get("name").and_then(|v| v.as_str()).unwrap_or(&filename).to_string();
            let id = filename.replace(".json", "").replace('-', "_");

            let robots = v.get("robots").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
            let suggested = v.get("suggested_agents").and_then(|v| v.as_u64()).map(|n| n as usize)
                .unwrap_or(robots);

            // Compute walkable cells
            let walkable = if let Some(wc) = v.get("walkable_cells").and_then(|v| v.as_u64()) {
                wc as usize
            } else if let Some(cells) = v.get("cells").and_then(|c| c.as_array()) {
                let walls = cells.iter().filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("wall")).count();
                (width * height) as usize - walls
            } else {
                (width * height) as usize
            };

            entries.push(TopologyEntry {
                id,
                name,
                width,
                height,
                suggested_agents: suggested,
                walkable_cells: walkable,
                json_data,
            });
        }

        Self { entries }
    }

    /// Parse a topology entry's JSON into (GridMap, ZoneMap) for use by the experiment runner
    /// or the custom map loader.
    pub fn parse_entry(entry: &TopologyEntry) -> Option<(GridMap, ZoneMap)> {
        let v: serde_json::Value = serde_json::from_str(&entry.json_data).ok()?;
        Self::parse_json_value(&v)
    }

    /// Parse a JSON value into (GridMap, ZoneMap).
    pub fn parse_json_value(v: &serde_json::Value) -> Option<(GridMap, ZoneMap)> {
        let width = v.get("width")?.as_i64()? as i32;
        let height = v.get("height")?.as_i64()? as i32;

        let mut obstacles = std::collections::HashSet::new();
        let mut zones = ZoneMap::default();
        let mut delivery_directions: Vec<(IVec2, Direction)> = Vec::new();

        if let Some(cells) = v.get("cells").and_then(|c| c.as_array()) {
            for cell in cells {
                let x = cell.get("x").and_then(|v| v.as_i64())? as i32;
                let y = cell.get("y").and_then(|v| v.as_i64())? as i32;
                let cell_type = cell.get("type").and_then(|v| v.as_str())?;
                let pos = IVec2::new(x, y);

                match cell_type {
                    "wall" => { obstacles.insert(pos); }
                    "pickup" => {
                        zones.pickup_cells.push(pos);
                        zones.zone_type.insert(pos, ZoneType::Pickup);
                    }
                    "delivery" => {
                        zones.delivery_cells.push(pos);
                        zones.zone_type.insert(pos, ZoneType::Delivery);
                        // Parse queue_direction if present
                        if let Some(dir_str) = cell.get("queue_direction").and_then(|d| d.as_str()) {
                            let dir = match dir_str {
                                "north" => Some(Direction::North),
                                "south" => Some(Direction::South),
                                "east" => Some(Direction::East),
                                "west" => Some(Direction::West),
                                _ => None,
                            };
                            if let Some(d) = dir {
                                delivery_directions.push((pos, d));
                            }
                        }
                    }
                    "recharging" => {
                        zones.recharging_cells.push(pos);
                        zones.zone_type.insert(pos, ZoneType::Recharging);
                    }
                    _ => {}
                }
            }
        }

        // All walkable non-zone cells are corridors
        for x in 0..width {
            for y in 0..height {
                let pos = IVec2::new(x, y);
                if !obstacles.contains(&pos) && !zones.zone_type.contains_key(&pos) {
                    zones.corridor_cells.push(pos);
                    zones.zone_type.insert(pos, ZoneType::Corridor);
                }
            }
        }

        let grid = GridMap::with_obstacles(width, height, obstacles);

        // Build queue lines from delivery cells with queue_direction
        if !delivery_directions.is_empty() {
            zones.queue_lines = build_queue_lines(&delivery_directions, &grid);
        }

        // Validate connectivity: all pickup/delivery cells must be reachable
        if let Err(unreachable) = validate_connectivity(&grid, &zones) {
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!(
                "Topology validation failed: {} zone cells are unreachable: {:?}",
                unreachable.len(),
                &unreachable[..unreachable.len().min(5)],
            );
            return None;
        }

        Some((grid, zones))
    }
}

// ---------------------------------------------------------------------------
// Connectivity validation
// ---------------------------------------------------------------------------

/// Validate that all pickup and delivery cells are reachable from each other.
///
/// Runs BFS from the first walkable cell and checks that every pickup and
/// delivery cell belongs to the same connected component. Returns `Ok(())`
/// if the map is valid, or `Err(unreachable)` listing zone cells that are
/// isolated from the main walkable area.
///
/// Maps with no pickup AND no delivery cells pass (custom/empty maps).
pub fn validate_connectivity(grid: &GridMap, zones: &ZoneMap) -> Result<(), Vec<IVec2>> {
    // If there are no zone cells at all, nothing to validate
    if zones.pickup_cells.is_empty() && zones.delivery_cells.is_empty() {
        return Ok(());
    }

    // Find the first walkable cell as BFS seed (prefer first pickup, then delivery, then any)
    let seed = zones.pickup_cells.first()
        .or(zones.delivery_cells.first())
        .copied()
        .or_else(|| {
            for y in 0..grid.height {
                for x in 0..grid.width {
                    let pos = IVec2::new(x, y);
                    if grid.is_walkable(pos) {
                        return Some(pos);
                    }
                }
            }
            None
        });

    let seed = match seed {
        Some(s) => s,
        None => return Ok(()), // No walkable cells at all — degenerate but not our problem
    };

    // BFS from seed
    let capacity = (grid.width * grid.height) as usize;
    let mut visited = vec![false; capacity];
    let mut queue = std::collections::VecDeque::new();

    let idx = |pos: IVec2| -> usize { (pos.y * grid.width + pos.x) as usize };

    visited[idx(seed)] = true;
    queue.push_back(seed);

    while let Some(pos) = queue.pop_front() {
        for neighbor in grid.walkable_neighbors(pos) {
            let ni = idx(neighbor);
            if !visited[ni] {
                visited[ni] = true;
                queue.push_back(neighbor);
            }
        }
    }

    // Check all pickup and delivery cells are in the reachable set
    let mut unreachable = Vec::new();
    for &pos in &zones.pickup_cells {
        if !visited[idx(pos)] {
            unreachable.push(pos);
        }
    }
    for &pos in &zones.delivery_cells {
        if !visited[idx(pos)] {
            unreachable.push(pos);
        }
    }

    if unreachable.is_empty() {
        Ok(())
    } else {
        Err(unreachable)
    }
}

// ---------------------------------------------------------------------------
// MovingAI .map file parser
// ---------------------------------------------------------------------------

/// Parse a MovingAI benchmark .map file into (GridMap, ZoneMap).
///
/// Format:
/// ```text
/// type octile
/// height 32
/// width 32
/// map
/// ....@@@@....
/// .....@......
/// ```
///
/// `.` and `G` = walkable. `@`, `T`, `O`, `W` = obstacle.
/// All walkable cells become corridors (no pickup/delivery zones).
/// For lifelong benchmarks, pickup/delivery zones should be assigned
/// separately via `assign_random_zones()`.
pub fn parse_movingai_map(text: &str) -> Option<(GridMap, ZoneMap)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut width: i32 = 0;
    let mut height: i32 = 0;
    let mut map_start: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("height") {
            height = trimmed.split_whitespace().nth(1)?.parse().ok()?;
        }
        if trimmed.starts_with("width") {
            width = trimmed.split_whitespace().nth(1)?.parse().ok()?;
        }
        if trimmed == "map" {
            map_start = Some(i + 1);
            break;
        }
    }

    let map_start = map_start?;
    if width <= 0 || height <= 0 {
        return None;
    }

    let mut obstacles = std::collections::HashSet::new();
    for y in 0..height {
        let line_idx = map_start + y as usize;
        if line_idx >= lines.len() {
            break;
        }
        let row = lines[line_idx];
        for (x, ch) in row.chars().enumerate() {
            if x as i32 >= width {
                break;
            }
            // '.' and 'G' are walkable. Everything else is obstacle.
            if ch != '.' && ch != 'G' {
                obstacles.insert(IVec2::new(x as i32, y as i32));
            }
        }
    }

    let grid = GridMap::with_obstacles(width, height, obstacles);

    // Build a basic ZoneMap: all walkable cells are corridors.
    // For lifelong MAPF benchmarks, call assign_random_zones() to designate
    // some cells as pickup/delivery.
    let mut corridor_cells = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let pos = IVec2::new(x, y);
            if grid.is_walkable(pos) {
                corridor_cells.push(pos);
            }
        }
    }

    let zones = ZoneMap {
        pickup_cells: Vec::new(),
        delivery_cells: Vec::new(),
        corridor_cells,
        recharging_cells: Vec::new(),
        zone_type: std::collections::HashMap::new(),
        queue_lines: Vec::new(),
    };

    Some((grid, zones))
}

/// Assign random pickup and delivery zones to a ZoneMap from corridor cells.
/// Takes the first `n_pickup` corridor cells as pickups and the last `n_delivery`
/// as deliveries (deterministic given the cell ordering).
pub fn assign_random_zones(zones: &mut ZoneMap, n_pickup: usize, n_delivery: usize) {
    if zones.corridor_cells.len() < n_pickup + n_delivery {
        return;
    }
    zones.pickup_cells = zones.corridor_cells[..n_pickup].to_vec();
    zones.delivery_cells = zones.corridor_cells[zones.corridor_cells.len() - n_delivery..].to_vec();
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct TopologyPlugin;

impl Plugin for TopologyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneMap>()
            .init_resource::<ActiveTopology>();

        // Load topology registry from topologies/ on desktop
        #[cfg(not(target_arch = "wasm32"))]
        {
            let topologies_dir = std::path::Path::new("topologies");
            let registry = TopologyRegistry::load_from_dir(topologies_dir);
            app.insert_resource(registry);
        }

        #[cfg(target_arch = "wasm32")]
        {
            app.init_resource::<TopologyRegistry>();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_map_default_is_empty() {
        let zm = ZoneMap::default();
        assert!(zm.pickup_cells.is_empty());
        assert!(zm.delivery_cells.is_empty());
        assert!(zm.corridor_cells.is_empty());
        assert!(zm.recharging_cells.is_empty());
        assert!(zm.zone_type.is_empty());
    }

    #[test]
    fn active_topology_default_loads_first_entry() {
        let at = ActiveTopology::default();
        // Should load the first topology from topologies/ directory
        assert!(!at.name().is_empty());
    }

    #[test]
    fn active_topology_from_unknown_falls_back() {
        let at = ActiveTopology::from_name("nonexistent");
        // Should fall back to first available topology
        assert!(!at.name().is_empty());
    }

    // ── Connectivity validation tests ───────────────────────────────

    #[test]
    fn validate_connected_map_passes() {
        use crate::core::grid::GridMap;
        let grid = GridMap::new(10, 10); // fully open
        let mut zones = ZoneMap::default();
        zones.pickup_cells.push(IVec2::new(1, 1));
        zones.delivery_cells.push(IVec2::new(8, 8));
        assert!(validate_connectivity(&grid, &zones).is_ok());
    }

    #[test]
    fn validate_disconnected_map_fails() {
        use crate::core::grid::GridMap;
        use std::collections::HashSet;
        // Wall splits grid vertically at x=5
        let mut obstacles = HashSet::new();
        for y in 0..10 {
            obstacles.insert(IVec2::new(5, y));
        }
        let grid = GridMap::with_obstacles(10, 10, obstacles);
        let mut zones = ZoneMap::default();
        zones.pickup_cells.push(IVec2::new(1, 1)); // left side
        zones.delivery_cells.push(IVec2::new(8, 8)); // right side
        let result = validate_connectivity(&grid, &zones);
        assert!(result.is_err());
        let unreachable = result.unwrap_err();
        assert!(unreachable.contains(&IVec2::new(8, 8)));
    }

    #[test]
    fn validate_island_pickup_fails() {
        use crate::core::grid::GridMap;
        use std::collections::HashSet;
        // Pickup at (5,5) surrounded by walls
        let mut obstacles = HashSet::new();
        for d in &[IVec2::new(0, 1), IVec2::new(0, -1), IVec2::new(1, 0), IVec2::new(-1, 0)] {
            obstacles.insert(IVec2::new(5, 5) + *d);
        }
        let grid = GridMap::with_obstacles(10, 10, obstacles);
        let mut zones = ZoneMap::default();
        zones.pickup_cells.push(IVec2::new(5, 5)); // isolated
        zones.delivery_cells.push(IVec2::new(0, 0)); // reachable
        let result = validate_connectivity(&grid, &zones);
        // The BFS seeds from the first pickup cell (5,5) which is isolated,
        // so delivery at (0,0) becomes unreachable
        assert!(result.is_err());
    }

    #[test]
    fn validate_island_delivery_fails() {
        use crate::core::grid::GridMap;
        use std::collections::HashSet;
        // Delivery at (5,5) surrounded by walls
        let mut obstacles = HashSet::new();
        for d in &[IVec2::new(0, 1), IVec2::new(0, -1), IVec2::new(1, 0), IVec2::new(-1, 0)] {
            obstacles.insert(IVec2::new(5, 5) + *d);
        }
        let grid = GridMap::with_obstacles(10, 10, obstacles);
        let mut zones = ZoneMap::default();
        zones.pickup_cells.push(IVec2::new(0, 0)); // reachable
        zones.delivery_cells.push(IVec2::new(5, 5)); // isolated
        let result = validate_connectivity(&grid, &zones);
        assert!(result.is_err());
        let unreachable = result.unwrap_err();
        assert!(unreachable.contains(&IVec2::new(5, 5)));
    }

    #[test]
    fn validate_no_zones_passes() {
        use crate::core::grid::GridMap;
        let grid = GridMap::new(10, 10);
        let zones = ZoneMap::default(); // no pickup, no delivery
        assert!(validate_connectivity(&grid, &zones).is_ok());
    }

    #[test]
    fn validate_existing_topologies_pass() {
        let registry = TopologyRegistry::load_from_dir(std::path::Path::new("topologies"));
        assert!(!registry.entries.is_empty(), "No topologies found in topologies/");
        for entry in &registry.entries {
            let result = TopologyRegistry::parse_entry(entry);
            assert!(
                result.is_some(),
                "Topology '{}' failed connectivity validation",
                entry.id,
            );
        }
    }

    // ── Original tests ──────────────────────────────────────────────

    #[test]
    fn custom_map_topology_preserves_data() {
        use crate::core::grid::GridMap;
        use std::collections::HashSet;

        let mut obstacles = HashSet::new();
        obstacles.insert(IVec2::new(2, 3));
        obstacles.insert(IVec2::new(5, 5));
        let grid = GridMap::with_obstacles(10, 10, obstacles);

        let mut zones = ZoneMap::default();
        zones.pickup_cells.push(IVec2::new(1, 1));
        zones.delivery_cells.push(IVec2::new(8, 8));
        zones.recharging_cells.push(IVec2::new(0, 0));
        zones.zone_type.insert(IVec2::new(1, 1), ZoneType::Pickup);
        zones.zone_type.insert(IVec2::new(8, 8), ZoneType::Delivery);
        zones.zone_type.insert(IVec2::new(0, 0), ZoneType::Recharging);

        let custom = CustomMap { grid, zones };
        assert_eq!(custom.name(), "custom");

        let output = custom.generate(0);
        assert_eq!(output.grid.width, 10);
        assert_eq!(output.grid.height, 10);
        assert!(output.grid.is_obstacle(IVec2::new(2, 3)));
        assert!(output.grid.is_obstacle(IVec2::new(5, 5)));
        assert!(!output.grid.is_obstacle(IVec2::new(0, 0)));
        assert_eq!(output.zones.pickup_cells.len(), 1);
        assert_eq!(output.zones.delivery_cells.len(), 1);
        assert_eq!(output.zones.recharging_cells.len(), 1);
        assert_eq!(output.zones.zone_type.get(&IVec2::new(0, 0)), Some(&ZoneType::Recharging));
    }
}
