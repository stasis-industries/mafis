//! Token Passing task selection (Ma, Li, Kumar, Koenig, AAMAS 2017, Alg. 1
//! lines 7-12; pibt2 `tp.cpp:64-96`).
//!
//! Used in place of the configured scheduler whenever the solver is Token
//! Passing (see `LifelongSolver::task_scheduler`). Task creation is the same
//! random pool as `ClosestFirstScheduler`; assignment follows TP:
//! - agents request the token in agent-index order (`tp.cpp:52`);
//! - a task is eligible only if no other agent's path ends at its pickup
//!   (`tp.cpp:69-73`) — path ends are the goals of all alive agents;
//! - among eligible tasks the agent takes the one with the nearest pickup
//!   (`tp.cpp:79-84`; Manhattan distance here, pibt2 uses BFS distance).

use bevy::prelude::IVec2;
use rand_chacha::ChaCha8Rng;
use std::collections::HashSet;

use super::super::topology::ZoneMap;
use super::{ClosestFirstScheduler, TaskScheduler};

pub struct TokenPassingTaskSelection;

impl TaskScheduler for TokenPassingTaskSelection {
    fn name(&self) -> &str {
        "token_passing"
    }

    fn assign_pickup(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        ClosestFirstScheduler.assign_pickup(zones, pos, occupied, rng)
    }

    fn assign_delivery(
        &self,
        zones: &ZoneMap,
        pos: IVec2,
        occupied: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Option<IVec2> {
        ClosestFirstScheduler.assign_delivery(zones, pos, occupied, rng)
    }

    fn assign_pickups_batch(
        &self,
        free_agents: &[(usize, IVec2)],
        zones: &ZoneMap,
        used_goals: &mut HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Vec<(usize, IVec2)> {
        self.assign_pickups_with_path_ends(free_agents, zones, used_goals, &HashSet::new(), rng)
    }

    fn assign_pickups_with_path_ends(
        &self,
        free_agents: &[(usize, IVec2)],
        zones: &ZoneMap,
        used_goals: &mut HashSet<IVec2>,
        path_ends: &HashSet<IVec2>,
        rng: &mut ChaCha8Rng,
    ) -> Vec<(usize, IVec2)> {
        let mut ordered = free_agents.to_vec();
        ordered.sort_by_key(|&(idx, _)| idx);
        let mut excluded: HashSet<IVec2> = used_goals.union(path_ends).copied().collect();
        let assignments =
            ClosestFirstScheduler.assign_pickups_batch(&ordered, zones, &mut excluded, rng);
        used_goals.extend(assignments.iter().map(|&(_, pickup)| pickup));
        assignments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::task::{TaskAgentSnapshot, TaskLeg, recycle_goals_core};
    use rand::SeedableRng;

    fn zones(pickups: &[IVec2]) -> ZoneMap {
        ZoneMap { pickup_cells: pickups.to_vec(), ..Default::default() }
    }

    fn agent(pos: IVec2, leg: TaskLeg) -> TaskAgentSnapshot {
        TaskAgentSnapshot { pos, goal: pos, task_leg: leg, alive: true, frozen: false }
    }

    fn pickup_of(leg: &TaskLeg) -> Option<IVec2> {
        match leg {
            TaskLeg::TravelEmpty(p) => Some(*p),
            _ => None,
        }
    }

    /// A pickup where another agent's path ends (here: an agent loading there)
    /// is never assigned (pibt2 tp.cpp:69-73).
    #[test]
    fn pickup_at_another_agents_path_end_is_not_assigned() {
        let a = IVec2::new(0, 0);
        let b = IVec2::new(9, 0);
        let z = zones(&[a, b]);
        let agents = [agent(a, TaskLeg::Loading(a)), agent(IVec2::new(1, 0), TaskLeg::Free)];
        for seed in 0..50 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let r = recycle_goals_core(&agents, &TokenPassingTaskSelection, &z, &mut rng, 1);
            assert_eq!(pickup_of(&r.updates[1].task_leg), Some(b), "seed {seed}");
        }
    }

    /// Agents request the token in index order (pibt2 tp.cpp:52), so the
    /// lower-index agent gets its nearest task even when the shuffle in
    /// `recycle_goals_core` would put the other agent first.
    #[test]
    fn agents_choose_in_index_order() {
        let near = IVec2::new(5, 0);
        let far = IVec2::new(20, 0);
        let z = zones(&[near, far]);
        // Agent 1 is closer to `near`, but agent 0 chooses first.
        let agents =
            [agent(IVec2::new(0, 0), TaskLeg::Free), agent(IVec2::new(4, 0), TaskLeg::Free)];
        for seed in 0..50 {
            let mut rng = ChaCha8Rng::seed_from_u64(seed);
            let r = recycle_goals_core(&agents, &TokenPassingTaskSelection, &z, &mut rng, 1);
            assert_eq!(pickup_of(&r.updates[0].task_leg), Some(near), "seed {seed}");
            assert_eq!(pickup_of(&r.updates[1].task_leg), Some(far), "seed {seed}");
        }
    }
}
