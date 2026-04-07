//! Robustness tests — property-based fuzzing for untrusted input and
//! handcrafted negative/error path tests for edge cases.
//!
//! Run: cargo test --test robustness

use std::collections::HashSet;

use bevy::math::IVec2;
use proptest::prelude::*;

use mafis::core::grid::GridMap;
use mafis::core::topology::{TopologyRegistry, ZoneMap, validate_connectivity};
use mafis::solver::lifelong_solver_from_name;

// ═══════════════════════════════════════════════════════════════════════════
// Section 1: Property-based fuzzing (proptest)
// ═══════════════════════════════════════════════════════════════════════════

// ── parse_json_value: should never panic on any input ──────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn parse_json_value_never_panics_on_random_string(s in "\\PC*") {
        // Random byte strings — should return None, never panic
        let v: Result<serde_json::Value, _> = serde_json::from_str(&s);
        if let Ok(v) = v {
            let _ = TopologyRegistry::parse_json_value(&v);
        }
    }

    #[test]
    fn parse_json_value_never_panics_on_random_json(
        width in prop::option::of(-100i64..10000),
        height in prop::option::of(-100i64..10000),
        num_cells in 0usize..50,
    ) {
        // Construct JSON with potentially invalid values
        let mut cells = Vec::new();
        for i in 0..num_cells {
            cells.push(serde_json::json!({
                "x": i as i64 - 10,
                "y": i as i64 - 5,
                "type": match i % 5 {
                    0 => "wall",
                    1 => "pickup",
                    2 => "delivery",
                    3 => "recharging",
                    _ => "unknown_type",
                }
            }));
        }

        let mut map = serde_json::Map::new();
        if let Some(w) = width {
            map.insert("width".into(), serde_json::json!(w));
        }
        if let Some(h) = height {
            map.insert("height".into(), serde_json::json!(h));
        }
        map.insert("cells".into(), serde_json::json!(cells));

        let v = serde_json::Value::Object(map);
        let _ = TopologyRegistry::parse_json_value(&v);
    }

    #[test]
    fn parse_json_value_never_panics_on_wrong_types(
        width_str in "[a-z]{1,10}",
        height_float in -1000.0f64..1000.0,
    ) {
        // Width as string, height as float — type mismatches
        let v = serde_json::json!({
            "width": width_str,
            "height": height_float,
            "cells": []
        });
        let result = TopologyRegistry::parse_json_value(&v);
        assert!(result.is_none(), "string width should fail");
    }

    #[test]
    fn parse_json_value_never_panics_on_missing_cell_fields(
        num_cells in 1usize..20,
    ) {
        // Cells with missing x, y, or type fields
        let cells: Vec<serde_json::Value> = (0..num_cells)
            .map(|i| match i % 4 {
                0 => serde_json::json!({"y": 0, "type": "wall"}),         // missing x
                1 => serde_json::json!({"x": 0, "type": "pickup"}),       // missing y
                2 => serde_json::json!({"x": 0, "y": 0}),                 // missing type
                _ => serde_json::json!({}),                                // empty
            })
            .collect();

        let v = serde_json::json!({
            "width": 10,
            "height": 10,
            "cells": cells
        });
        let _ = TopologyRegistry::parse_json_value(&v);
    }

    #[test]
    fn parse_json_value_never_panics_on_huge_dimensions(
        width in 0i64..100000,
        height in 0i64..100000,
    ) {
        // Guard: skip truly massive allocations (>100M cells)
        if width * height > 1_000_000 {
            return Ok(());
        }
        let v = serde_json::json!({
            "width": width,
            "height": height,
            "cells": []
        });
        let _ = TopologyRegistry::parse_json_value(&v);
    }
}

// ── validate_connectivity: should never panic on any grid ──────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn validate_connectivity_never_panics(
        width in 1i32..30,
        height in 1i32..30,
        obstacle_ratio in 0.0f64..1.0,
        num_pickups in 0usize..5,
        num_deliveries in 0usize..5,
        seed in 0u64..10000,
    ) {
        use rand::SeedableRng;
        use rand::Rng;
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);

        let mut obstacles = HashSet::new();
        for x in 0..width {
            for y in 0..height {
                if rng.random::<f64>() < obstacle_ratio {
                    obstacles.insert(IVec2::new(x, y));
                }
            }
        }
        let grid = GridMap::with_obstacles(width, height, obstacles.clone());

        let mut zones = ZoneMap::default();
        // Place pickups/deliveries at random cells (may overlap obstacles)
        for _ in 0..num_pickups {
            let pos = IVec2::new(rng.random_range(0..width), rng.random_range(0..height));
            zones.pickup_cells.push(pos);
        }
        for _ in 0..num_deliveries {
            let pos = IVec2::new(rng.random_range(0..width), rng.random_range(0..height));
            zones.delivery_cells.push(pos);
        }

        // Should never panic regardless of input
        let _ = validate_connectivity(&grid, &zones);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 2: Handcrafted negative / error path tests
// ═══════════════════════════════════════════════════════════════════════════

// ── parse_json_value edge cases ────────────────────────────────────────

#[test]
fn parse_json_value_empty_object() {
    let v = serde_json::json!({});
    assert!(TopologyRegistry::parse_json_value(&v).is_none());
}

#[test]
fn parse_json_value_null() {
    let v = serde_json::Value::Null;
    assert!(TopologyRegistry::parse_json_value(&v).is_none());
}

#[test]
fn parse_json_value_array_not_object() {
    let v = serde_json::json!([1, 2, 3]);
    assert!(TopologyRegistry::parse_json_value(&v).is_none());
}

#[test]
fn parse_json_value_zero_dimensions() {
    let v = serde_json::json!({"width": 0, "height": 0, "cells": []});
    // Width/height 0 should not panic
    let _ = TopologyRegistry::parse_json_value(&v);
}

#[test]
fn parse_json_value_negative_dimensions() {
    let v = serde_json::json!({"width": -5, "height": -3, "cells": []});
    let _ = TopologyRegistry::parse_json_value(&v);
}

#[test]
fn parse_json_value_missing_cells_array() {
    let v = serde_json::json!({"width": 10, "height": 10});
    // Missing "cells" key — should produce a grid with no obstacles and no zones
    let result = TopologyRegistry::parse_json_value(&v);
    if let Some((grid, zones)) = result {
        assert_eq!(grid.width, 10);
        assert!(zones.pickup_cells.is_empty());
        assert!(zones.delivery_cells.is_empty());
    }
}

#[test]
fn parse_json_value_cells_not_array() {
    let v = serde_json::json!({"width": 10, "height": 10, "cells": "not an array"});
    // "cells" is a string, not array — should silently skip
    let _ = TopologyRegistry::parse_json_value(&v);
}

#[test]
fn parse_json_value_cell_coords_outside_grid() {
    let v = serde_json::json!({
        "width": 5,
        "height": 5,
        "cells": [
            {"x": 100, "y": 100, "type": "wall"},
            {"x": -1, "y": -1, "type": "pickup"},
            {"x": 2, "y": 2, "type": "delivery"}
        ]
    });
    // Out-of-bounds cells should not crash
    let _ = TopologyRegistry::parse_json_value(&v);
}

#[test]
fn parse_json_value_duplicate_cells() {
    let v = serde_json::json!({
        "width": 5,
        "height": 5,
        "cells": [
            {"x": 2, "y": 2, "type": "pickup"},
            {"x": 2, "y": 2, "type": "pickup"},
            {"x": 2, "y": 2, "type": "delivery"}
        ]
    });
    // Duplicate cells should not crash
    let _ = TopologyRegistry::parse_json_value(&v);
}

// ── validate_connectivity edge cases ───────────────────────────────────

#[test]
fn connectivity_all_obstacles() {
    let mut obstacles = HashSet::new();
    for x in 0..10 {
        for y in 0..10 {
            obstacles.insert(IVec2::new(x, y));
        }
    }
    let grid = GridMap::with_obstacles(10, 10, obstacles);
    let mut zones = ZoneMap::default();
    zones.pickup_cells.push(IVec2::new(5, 5));
    zones.delivery_cells.push(IVec2::new(1, 1));

    // All cells are obstacles — zones are on obstacles, BFS has no walkable seed
    let result = validate_connectivity(&grid, &zones);
    // Should not panic. Returns Ok (no walkable seed) or Err (unreachable zones).
    let _ = result;
}

#[test]
fn connectivity_empty_zones_passes() {
    let grid = GridMap::new(10, 10);
    let zones = ZoneMap::default();
    assert!(validate_connectivity(&grid, &zones).is_ok());
}

#[test]
fn connectivity_single_cell_grid() {
    let grid = GridMap::new(1, 1);
    let mut zones = ZoneMap::default();
    zones.pickup_cells.push(IVec2::new(0, 0));
    zones.delivery_cells.push(IVec2::new(0, 0));
    assert!(validate_connectivity(&grid, &zones).is_ok());
}

#[test]
fn connectivity_zone_on_obstacle_unreachable_from_other_zones() {
    // Pickup at (1,1) is walkable. Delivery at (3,3) is on an obstacle.
    // BFS seeds from (1,1), cannot reach (3,3) because it's an obstacle.
    let mut obstacles = HashSet::new();
    obstacles.insert(IVec2::new(3, 3));
    let grid = GridMap::with_obstacles(10, 10, obstacles);
    let mut zones = ZoneMap::default();
    zones.pickup_cells.push(IVec2::new(1, 1));
    zones.delivery_cells.push(IVec2::new(3, 3)); // on obstacle — unreachable

    let result = validate_connectivity(&grid, &zones);
    assert!(result.is_err());
    let unreachable = result.unwrap_err();
    assert!(unreachable.contains(&IVec2::new(3, 3)));
}

// ── Grid edge cases ────────────────────────────────────────────────────

#[test]
fn grid_all_obstacles_no_walkable() {
    let mut obstacles = HashSet::new();
    for x in 0..5 {
        for y in 0..5 {
            obstacles.insert(IVec2::new(x, y));
        }
    }
    let grid = GridMap::with_obstacles(5, 5, obstacles);

    // No cell should be walkable
    for x in 0..5 {
        for y in 0..5 {
            assert!(!grid.is_walkable(IVec2::new(x, y)));
        }
    }
}

#[test]
fn grid_out_of_bounds_queries() {
    let grid = GridMap::new(5, 5);
    assert!(!grid.is_in_bounds(IVec2::new(-1, 0)));
    assert!(!grid.is_in_bounds(IVec2::new(0, -1)));
    assert!(!grid.is_in_bounds(IVec2::new(5, 0)));
    assert!(!grid.is_in_bounds(IVec2::new(0, 5)));
    assert!(!grid.is_walkable(IVec2::new(-1, -1)));
    assert!(!grid.is_obstacle(IVec2::new(100, 100)));
}

// ── Solver factory edge cases ──────────────────────────────────────────

#[test]
fn solver_factory_unknown_name_returns_none() {
    assert!(lifelong_solver_from_name("nonexistent", 100, 10).is_none());
}

#[test]
fn solver_factory_empty_name_returns_none() {
    assert!(lifelong_solver_from_name("", 100, 10).is_none());
}

#[test]
fn solver_factory_zero_agents() {
    // All solvers should accept 0 agents without panic
    for name in ["pibt", "rhcr_pbs", "token_passing", "lacam3_lifelong"] {
        let solver = lifelong_solver_from_name(name, 100, 0);
        assert!(solver.is_some(), "{name} should accept 0 agents");
    }
}

#[test]
fn solver_factory_zero_grid_area() {
    // grid_area=0 might cause division by zero in auto-config
    // Should not panic
    for name in ["pibt", "rhcr_pbs", "token_passing", "lacam3_lifelong"] {
        let _ = lifelong_solver_from_name(name, 0, 10);
    }
}

#[test]
fn solver_factory_agents_exceed_grid() {
    // More agents than could possibly fit — factory shouldn't care
    for name in ["pibt", "rhcr_pbs", "token_passing", "lacam3_lifelong"] {
        let solver = lifelong_solver_from_name(name, 4, 1000);
        assert!(solver.is_some(), "{name} should accept large agent count at factory level");
    }
}

// ── Agent placement edge cases ─────────────────────────────────────────

#[test]
fn place_agents_on_fully_blocked_grid() {
    use mafis::analysis::baseline::place_agents;
    use mafis::core::seed::SeededRng;

    let mut obstacles = HashSet::new();
    for x in 0..5 {
        for y in 0..5 {
            obstacles.insert(IVec2::new(x, y));
        }
    }
    let grid = GridMap::with_obstacles(5, 5, obstacles);
    let zones = ZoneMap::default();
    let mut rng = SeededRng::new(42);

    // No walkable cells — should not panic
    let agents = place_agents(3, &grid, &zones, &mut rng);
    assert_eq!(agents.len(), 3);
}

#[test]
fn place_agents_zero_count() {
    use mafis::analysis::baseline::place_agents;
    use mafis::core::seed::SeededRng;

    let grid = GridMap::new(10, 10);
    let zones = ZoneMap::default();
    let mut rng = SeededRng::new(42);

    let agents = place_agents(0, &grid, &zones, &mut rng);
    assert!(agents.is_empty());
}

#[test]
fn place_agents_more_than_free_cells() {
    use mafis::analysis::baseline::place_agents;
    use mafis::core::seed::SeededRng;

    // 2x2 grid, no obstacles = 4 free cells, request 10 agents
    let grid = GridMap::new(2, 2);
    let zones = ZoneMap::default();
    let mut rng = SeededRng::new(42);

    // Should not panic — excess agents get fallback placement
    let agents = place_agents(10, &grid, &zones, &mut rng);
    assert_eq!(agents.len(), 10);
}

// ── Scheduler factory edge cases ───────────────────────────────────────

#[test]
fn scheduler_factory_unknown_name_defaults() {
    use mafis::core::task::ActiveScheduler;

    let s = ActiveScheduler::from_name("garbage");
    assert_eq!(s.name(), "random", "unknown scheduler should default to random");
}

#[test]
fn scheduler_factory_empty_name_defaults() {
    use mafis::core::task::ActiveScheduler;

    let s = ActiveScheduler::from_name("");
    assert_eq!(s.name(), "random");
}
