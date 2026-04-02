//! Zero-alloc collision resolution buffers.

use bevy::math::IVec2;

use crate::core::action::Action;

use super::sim_agent::SimAgent;

// ---------------------------------------------------------------------------
// CollisionBuffers — flat grid-indexed arrays for zero-alloc collision resolution
// ---------------------------------------------------------------------------

/// Sentinel: no agent occupies this cell.
pub(super) const COLLISION_NO_AGENT: u32 = u32::MAX;

#[derive(Default)]
pub(super) struct CollisionBuffers {
    /// Per-cell: which agent targets this cell (for vertex conflict detection).
    /// `COLLISION_NO_AGENT` = vacant.
    pub target_agent: Vec<u32>,
    /// Per-cell: how many agents target this cell.
    pub target_count: Vec<u16>,
    /// Per-cell: source agent moving FROM this cell (for edge swap detection).
    pub source_agent: Vec<u32>,
    /// Per-cell: whether a dead agent occupies this cell (O(1) lookup).
    pub dead_cell: Vec<bool>,
    /// Dirty cell indices — only these need clearing between iterations.
    pub dirty_targets: Vec<usize>,
    pub dirty_sources: Vec<usize>,
    pub dirty_dead: Vec<usize>,
    /// Collision moves buffer: (current_pos, action, target_pos, was_forced).
    pub moves: Vec<(IVec2, Action, IVec2, bool)>,
    /// Grid dimensions.
    pub grid_w: i32,
    pub grid_size: usize,
}

impl CollisionBuffers {
    pub fn new() -> Self {
        Self {
            target_agent: Vec::new(),
            target_count: Vec::new(),
            source_agent: Vec::new(),
            dead_cell: Vec::new(),
            dirty_targets: Vec::new(),
            dirty_sources: Vec::new(),
            dirty_dead: Vec::new(),
            moves: Vec::new(),
            grid_w: 0,
            grid_size: 0,
        }
    }

    /// Ensure buffers are sized for the grid. Only reallocates on grid change.
    pub fn ensure_size(&mut self, grid_w: i32, grid_h: i32) {
        let size = (grid_w * grid_h) as usize;
        if self.grid_size != size {
            self.grid_w = grid_w;
            self.grid_size = size;
            self.target_agent.clear();
            self.target_agent.resize(size, COLLISION_NO_AGENT);
            self.target_count.clear();
            self.target_count.resize(size, 0);
            self.source_agent.clear();
            self.source_agent.resize(size, COLLISION_NO_AGENT);
            self.dead_cell.clear();
            self.dead_cell.resize(size, false);
            self.dirty_targets.clear();
            self.dirty_sources.clear();
            self.dirty_dead.clear();
        }
    }

    #[inline]
    pub fn idx(&self, pos: IVec2) -> usize {
        (pos.y * self.grid_w + pos.x) as usize
    }

    /// Clear only the dirty cells (lazy clear — O(agents) instead of O(grid)).
    pub fn clear_targets(&mut self) {
        for &i in &self.dirty_targets {
            self.target_agent[i] = COLLISION_NO_AGENT;
            self.target_count[i] = 0;
        }
        self.dirty_targets.clear();
    }

    pub fn clear_sources(&mut self) {
        for &i in &self.dirty_sources {
            self.source_agent[i] = COLLISION_NO_AGENT;
        }
        self.dirty_sources.clear();
    }

    pub fn clear_dead(&mut self) {
        for &i in &self.dirty_dead {
            self.dead_cell[i] = false;
        }
        self.dirty_dead.clear();
    }
}

/// Count alive agents (excluding `skip_agent`) whose planned paths pass through `cell`.
/// Free function to avoid borrow conflicts when called inside `&mut self` methods.
pub(super) fn count_paths_through_cell(agents: &[SimAgent], cell: IVec2, skip_agent: usize) -> u32 {
    let mut count = 0u32;
    for (i, agent) in agents.iter().enumerate() {
        if i == skip_agent || !agent.alive || agent.planned_path.is_empty() {
            continue;
        }
        let mut pos = agent.pos;
        for action in &agent.planned_path {
            pos = action.apply(pos);
            if pos == cell {
                count += 1;
                break;
            }
        }
    }
    count
}
