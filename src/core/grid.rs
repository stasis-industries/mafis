use bevy::prelude::*;
use std::collections::HashSet;

use super::action::Direction;

/// Grid map with dual storage: flat `Vec<bool>` for O(1) walkability checks
/// in pathfinding hot paths, plus `HashSet<IVec2>` for obstacle enumeration
/// in cold paths (rendering, serialization, UI).
#[derive(Resource, Debug, Clone)]
pub struct GridMap {
    pub width: i32,
    pub height: i32,
    /// Flat grid indexed as `y * width + x`. `true` = obstacle.
    cells: Vec<bool>,
    /// Obstacle positions for callers that need iteration/cloning.
    obstacle_set: HashSet<IVec2>,
}

impl GridMap {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            cells: vec![false; (width * height) as usize],
            obstacle_set: HashSet::new(),
        }
    }

    pub fn with_obstacles(width: i32, height: i32, obstacles: HashSet<IVec2>) -> Self {
        let mut grid = Self::new(width, height);
        for pos in obstacles {
            grid.set_obstacle(pos);
        }
        grid
    }

    #[inline]
    fn idx(&self, pos: IVec2) -> usize {
        (pos.y * self.width + pos.x) as usize
    }

    #[inline]
    pub fn is_in_bounds(&self, pos: IVec2) -> bool {
        pos.x >= 0 && pos.x < self.width && pos.y >= 0 && pos.y < self.height
    }

    #[inline]
    pub fn is_obstacle(&self, pos: IVec2) -> bool {
        self.is_in_bounds(pos) && self.cells[self.idx(pos)]
    }

    #[inline]
    pub fn is_walkable(&self, pos: IVec2) -> bool {
        self.is_in_bounds(pos) && !self.cells[self.idx(pos)]
    }

    pub fn set_obstacle(&mut self, pos: IVec2) {
        if self.is_in_bounds(pos) {
            let i = self.idx(pos);
            if !self.cells[i] {
                self.cells[i] = true;
                self.obstacle_set.insert(pos);
            }
        }
    }

    pub fn remove_obstacle(&mut self, pos: IVec2) {
        if self.is_in_bounds(pos) {
            let i = self.idx(pos);
            if self.cells[i] {
                self.cells[i] = false;
                self.obstacle_set.remove(&pos);
            }
        }
    }

    pub fn obstacles(&self) -> &HashSet<IVec2> {
        &self.obstacle_set
    }

    /// Number of obstacle cells. O(1).
    #[inline]
    pub fn obstacle_count(&self) -> usize {
        self.obstacle_set.len()
    }

    pub fn walkable_neighbors(&self, pos: IVec2) -> Vec<IVec2> {
        Direction::ALL
            .iter()
            .map(|dir| pos + dir.offset())
            .filter(|neighbor| self.is_walkable(*neighbor))
            .collect()
    }

    pub fn cell_count(&self) -> i32 {
        self.width * self.height
    }

    pub fn walkable_count(&self) -> usize {
        (self.cell_count() as usize).saturating_sub(self.obstacle_set.len())
    }
}

impl Default for GridMap {
    fn default() -> Self {
        Self::new(16, 16)
    }
}

pub struct GridPlugin;

impl Plugin for GridPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridMap>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_grid() -> GridMap {
        GridMap::new(5, 5)
    }

    #[test]
    fn new_grid_has_no_obstacles() {
        let g = open_grid();
        for x in 0..5 {
            for y in 0..5 {
                assert!(g.is_walkable(IVec2::new(x, y)));
            }
        }
    }

    #[test]
    fn cell_count_is_width_times_height() {
        assert_eq!(GridMap::new(4, 7).cell_count(), 28);
    }

    #[test]
    fn out_of_bounds_is_not_walkable() {
        let g = open_grid();
        assert!(!g.is_walkable(IVec2::new(-1, 0)));
        assert!(!g.is_walkable(IVec2::new(0, -1)));
        assert!(!g.is_walkable(IVec2::new(5, 0)));
        assert!(!g.is_walkable(IVec2::new(0, 5)));
    }

    #[test]
    fn set_obstacle_blocks_walkability() {
        let mut g = open_grid();
        let pos = IVec2::new(2, 3);
        g.set_obstacle(pos);
        assert!(!g.is_walkable(pos));
        assert!(g.is_obstacle(pos));
    }

    #[test]
    fn remove_obstacle_restores_walkability() {
        let mut g = open_grid();
        let pos = IVec2::new(1, 1);
        g.set_obstacle(pos);
        g.remove_obstacle(pos);
        assert!(g.is_walkable(pos));
        assert!(!g.is_obstacle(pos));
    }

    #[test]
    fn walkable_neighbors_center_has_four() {
        let g = open_grid();
        let neighbors = g.walkable_neighbors(IVec2::new(2, 2));
        assert_eq!(neighbors.len(), 4);
    }

    #[test]
    fn walkable_neighbors_corner_has_two() {
        let g = open_grid();
        let neighbors = g.walkable_neighbors(IVec2::ZERO);
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn walkable_neighbors_excludes_obstacles() {
        let mut g = open_grid();
        g.set_obstacle(IVec2::new(2, 3)); // north of center
        let neighbors = g.walkable_neighbors(IVec2::new(2, 2));
        assert_eq!(neighbors.len(), 3);
        assert!(!neighbors.contains(&IVec2::new(2, 3)));
    }

    #[test]
    fn default_grid_is_16x16() {
        let g = GridMap::default();
        assert_eq!(g.width, 16);
        assert_eq!(g.height, 16);
        assert_eq!(g.cell_count(), 256);
    }

    #[test]
    fn obstacle_count_tracks_correctly() {
        let mut g = open_grid();
        assert_eq!(g.obstacle_count(), 0);
        g.set_obstacle(IVec2::new(1, 1));
        assert_eq!(g.obstacle_count(), 1);
        g.set_obstacle(IVec2::new(1, 1)); // duplicate
        assert_eq!(g.obstacle_count(), 1);
        g.set_obstacle(IVec2::new(2, 2));
        assert_eq!(g.obstacle_count(), 2);
        g.remove_obstacle(IVec2::new(1, 1));
        assert_eq!(g.obstacle_count(), 1);
    }

    #[test]
    fn with_obstacles_populates_both_storages() {
        let mut obs = HashSet::new();
        obs.insert(IVec2::new(1, 1));
        obs.insert(IVec2::new(3, 3));
        let g = GridMap::with_obstacles(5, 5, obs);
        assert!(g.is_obstacle(IVec2::new(1, 1)));
        assert!(g.is_obstacle(IVec2::new(3, 3)));
        assert!(!g.is_walkable(IVec2::new(1, 1)));
        assert_eq!(g.obstacle_count(), 2);
        assert_eq!(g.obstacles().len(), 2);
    }
}
