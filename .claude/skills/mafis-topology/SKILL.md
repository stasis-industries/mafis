---
name: mafis-topology
description: >
  Guide for adding and modifying map topologies in MAFIS. Use this skill when the user
  wants to add a new topology, create a new map layout, modify the warehouse geometry, change
  zone generation, add a corridor or sorting center layout, adjust obstacle placement, work on
  the Topology trait, modify ZoneMap, or change how maps are generated. Also trigger for: "new
  map", "add a layout", "modify warehouse", "change the grid", "zone types", "map generation",
  "custom map", "grid size", "obstacle density", or any work in topology.rs, warehouse.rs, or
  open_floor.rs. This skill contains the Topology trait contract, zone system, generation
  patterns, warehouse geometry constraints, and the full checklist for adding new topologies.
---

# MAFIS Topology Guide

Topologies define the physical environment: grid layout, obstacle placement, and zone assignments.
Each topology generates a `GridMap` + `ZoneMap` from a seed, ensuring deterministic generation.

## Architecture

```
src/core/
├── topology.rs       ← Topology trait, TopologyOutput, ActiveTopology, from_name()
├── warehouse.rs      ← WarehouseTopology (3 presets)
└── open_floor.rs     ← OpenFloorTopology (random obstacles)
```

## Topology Trait

```rust
pub trait Topology: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn generate(&self, seed: u64) -> TopologyOutput;
}

pub struct TopologyOutput {
    pub grid: GridMap,
    pub zones: ZoneMap,
    pub suggested_agents: usize,
}
```

Every topology must produce:
1. A `GridMap` with walkable/obstacle cells
2. A `ZoneMap` with pickup, delivery, corridor, and other zone assignments
3. A suggested agent count for the layout

## ZoneMap & Zone Types

```rust
pub struct ZoneMap {
    pub pickup_cells: Vec<IVec2>,
    pub delivery_cells: Vec<IVec2>,
    pub corridor_cells: Vec<IVec2>,
    pub recharging_cells: Vec<IVec2>,
    pub zone_type: HashMap<IVec2, ZoneType>,
    pub queue_lines: Vec<...>,
}

pub enum ZoneType {
    Storage,      // Warehouse shelves — agents pick items here
    Pickup,       // Pickup stations
    Delivery,     // Drop-off stations
    Corridor,     // Movement lanes
    CrossAisle,   // Perpendicular connections between corridors
    Open,         // Unzoned walkable space
    Recharging,   // Recharging stations
}
```

The task scheduler uses `pickup_cells` and `delivery_cells` to assign goals. If these are empty, agents have nowhere to go — the simulation stalls.

## Existing Topologies

### WarehouseTopology (warehouse.rs)

Rectangular grid with parallel storage rows, delivery zone at the bottom, corridors between rows, and cross-aisles at regular intervals.

| Preset | Dimensions | Agents | Character |
|--------|-----------|--------|-----------|
| small | 20x12 | 8 | Quick testing |
| medium | 40x21 | 30 | Standard experiments |
| large | 70x33 | 80 | Stress testing |

**Geometry rules:**
- `WAREHOUSE_DELIVERY_DEPTH = 3` — delivery zone is 3 rows deep at bottom
- `WAREHOUSE_MODULE_HEIGHT = 3` — each storage module is 3 cells tall
- Heights snap to `DELIVERY_DEPTH + N*3` to align modules cleanly
- Cross-aisles every `cross_aisle_interval` columns (default: 8)
- Left/right corridors are always 1 cell wide
- Top row is always corridor

### OpenFloorTopology (open_floor.rs)

Random obstacles scattered on a flat grid. All walkable cells are `ZoneType::Open`. Pickup and delivery zones are randomly selected from walkable cells.

Default: 32x32, density parameter controls obstacle percentage.

### CustomMap

`CustomMap` wrapper exists for loading arbitrary grids (e.g., MovingAI benchmarks). Not wired into `from_name()` — used programmatically. MAX_GRID_DIM = 512 supports MovingAI map sizes.

## Topology Factory

```rust
pub fn from_name(name: &str) -> Self {
    match name {
        "warehouse_small"  => WarehouseTopology::small(),
        "warehouse_medium" => WarehouseTopology::medium(),
        "warehouse_large"  => WarehouseTopology::large(),
        _ => WarehouseTopology::small(),  // default fallback
    }
}
```

To add a new topology, register it here.

## Adding a New Topology — Checklist

### 1. Create the file

`src/core/your_topology.rs`:

```rust
use super::topology::*;
use super::grid::GridMap;
use crate::core::seed::SeededRng;

pub struct YourTopology {
    pub width: i32,
    pub height: i32,
}

impl YourTopology {
    pub fn new(width: i32, height: i32) -> Self { ... }
    pub fn small() -> Self { Self::new(20, 15) }
    pub fn medium() -> Self { Self::new(40, 25) }
}

impl Topology for YourTopology {
    fn name(&self) -> &'static str { "your_topology" }

    fn generate(&self, seed: u64) -> TopologyOutput {
        let mut rng = SeededRng::new(seed);
        let mut grid = GridMap::new(self.width, self.height);
        let mut zones = ZoneMap::default();

        // 1. Place obstacles
        // 2. Assign zones (pickup, delivery, corridor, recharging)
        // 3. Ensure connectivity (all walkable cells reachable)

        TopologyOutput {
            grid,
            zones,
            suggested_agents: self.width as usize / 3,
        }
    }
}
```

### 2. Register in the factory

In `src/core/topology.rs`, add to `from_name()`:
```rust
"your_topology" | "your_topology_small" => Box::new(YourTopology::small()),
```

### 3. Add module

In `src/core/mod.rs`:
```rust
pub mod your_topology;
```

### 4. Bridge / UI integration

- **WASM**: Bridge already handles any registered topology name via `set_topology "name"`.
- **Desktop**: Add entry in `src/ui/desktop/panels/simulation.rs` topology picker.

### 5. Write tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_valid_grid() {
        let out = YourTopology::small().generate(42);
        assert!(out.grid.width() > 0 && out.grid.height() > 0);
    }

    #[test]
    fn has_pickup_and_delivery_zones() {
        let out = YourTopology::small().generate(42);
        assert!(!out.zones.pickup_cells.is_empty());
        assert!(!out.zones.delivery_cells.is_empty());
    }

    #[test]
    fn all_zones_are_walkable() {
        let out = YourTopology::small().generate(42);
        for &cell in &out.zones.pickup_cells {
            assert!(out.grid.is_walkable(cell));
        }
    }

    #[test]
    fn deterministic_generation() {
        let a = YourTopology::small().generate(42);
        let b = YourTopology::small().generate(42);
        assert_eq!(a.zones.pickup_cells, b.zones.pickup_cells);
    }
}
```

### 6. Constants

Add topology-specific constants to `src/constants.rs`.

## Critical Rules

1. **Deterministic** — Same seed must produce identical output. Use `SeededRng` only.
2. **Connectivity** — All walkable cells must be reachable. A disconnected grid causes agent deadlocks.
3. **Non-empty zones** — `pickup_cells` and `delivery_cells` must never be empty.
4. **Walkable zones** — Every zone cell must be walkable in the grid.
5. **Suggested agents** — Must fit on walkable cells. `suggested_agents <= walkable_count`.

## Common Topology Ideas

| Topology | Description | Zone Strategy |
|----------|-------------|---------------|
| Sorting Center | Wide input belt → sorting lanes → output docks | Pickup = input belt, Delivery = output docks |
| Corridor Grid | Regular grid of perpendicular corridors | Pickup/Delivery at intersections |
| Hub-and-Spoke | Central hub with radiating corridors | Pickup = spokes, Delivery = hub |
| Multi-Floor | Stacked grids with elevator cells | Same as single floor per level |
