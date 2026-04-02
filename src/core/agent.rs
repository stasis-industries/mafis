use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

use super::action::Action;
use super::task::TaskLeg;

#[derive(Component, Debug)]
pub struct LogicalAgent {
    pub current_pos: IVec2,
    pub goal_pos: IVec2,
    pub planned_path: VecDeque<Action>,
    /// Length of the runner's planned path — kept in sync by `sync_runner_to_ecs`
    /// without cloning the full VecDeque every tick.
    pub path_length: usize,
    pub task_leg: TaskLeg,
}

impl LogicalAgent {
    pub fn new(start: IVec2, goal: IVec2) -> Self {
        Self {
            current_pos: start,
            goal_pos: goal,
            planned_path: VecDeque::new(),
            path_length: 0,
            task_leg: TaskLeg::Free,
        }
    }

    pub fn has_reached_goal(&self) -> bool {
        self.current_pos == self.goal_pos
    }

    pub fn has_plan(&self) -> bool {
        self.path_length > 0
    }
}

/// Per-agent action statistics for fault metrics (idle ratio, utilization).
#[derive(Component, Debug, Default, Clone)]
pub struct AgentActionStats {
    pub total_actions: u32,
    pub wait_actions: u32,
    pub move_actions: u32,
}

impl AgentActionStats {
    /// Fraction of actions that were waits (0.0–1.0).
    pub fn idle_ratio(&self) -> f32 {
        if self.total_actions == 0 {
            0.0
        } else {
            self.wait_actions as f32 / self.total_actions as f32
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
pub struct LastAction(pub Action);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentIndex(pub usize);

#[derive(Resource, Debug, Default)]
pub struct AgentRegistry {
    entity_to_index: HashMap<Entity, usize>,
    index_to_entity: HashMap<usize, Entity>,
    next_index: usize,
}

impl AgentRegistry {
    pub fn register(&mut self, entity: Entity) -> AgentIndex {
        let index = self.next_index;
        self.next_index += 1;
        self.entity_to_index.insert(entity, index);
        self.index_to_entity.insert(index, entity);
        AgentIndex(index)
    }

    pub fn get_entity(&self, index: AgentIndex) -> Option<Entity> {
        self.index_to_entity.get(&index.0).copied()
    }

    pub fn get_index(&self, entity: Entity) -> Option<AgentIndex> {
        self.entity_to_index.get(&entity).map(|&i| AgentIndex(i))
    }

    pub fn count(&self) -> usize {
        self.entity_to_index.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (AgentIndex, Entity)> + '_ {
        self.index_to_entity.iter().map(|(&i, &e)| (AgentIndex(i), e))
    }

    pub fn clear(&mut self) {
        self.entity_to_index.clear();
        self.index_to_entity.clear();
        self.next_index = 0;
    }
}

pub struct AgentPlugin;

impl Plugin for AgentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AgentRegistry>();
    }
}
