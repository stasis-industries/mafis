//! The planner registry: what each planner is, the parameters it declares,
//! and how to build it. The list itself lives in `crate::planners`; this
//! module only reads it, so shared code never names a planner.

use std::fmt;

use crate::core::topology::ZoneMap;
use crate::planning::params::{self, ParamError, ParamSpec, ParamValues, ResolvedParams};
use crate::planning::planner::Planner;

/// What a planner is, for pickers, result records and validation.
#[derive(Debug, Clone, Copy)]
pub struct PlannerInfo {
    pub id: &'static str,
    pub label: &'static str,
    /// One line on how the planner works.
    pub summary: &'static str,
    /// The citation it is ported from.
    pub reference: &'static str,
    pub params: &'static [ParamSpec],
    /// Picks its own tasks (`Planner::task_scheduler`) instead of using the
    /// configured scheduler.
    pub assigns_tasks: bool,
}

/// One registered planner.
pub struct PlannerEntry {
    pub info: PlannerInfo,
    /// Builds the planner for a run with `fleet` agents.
    pub build: fn(&ResolvedParams, usize) -> Box<dyn Planner>,
    /// Largest fleet the planner supports on a map; `None` when unbounded.
    pub fleet_limit: fn(&ZoneMap) -> Option<usize>,
    /// The planner's Rust type, for the naming guard.
    pub type_name: fn() -> &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlannerError {
    UnknownPlanner { id: String },
    Param { planner: &'static str, error: ParamError },
}

impl fmt::Display for PlannerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlannerError::UnknownPlanner { id } => write!(f, "unknown planner '{id}'"),
            PlannerError::Param { planner, error } => write!(f, "planner '{planner}': {error}"),
        }
    }
}

/// Fleet limit of a planner that runs any fleet.
pub fn no_fleet_limit(_zones: &ZoneMap) -> Option<usize> {
    None
}

/// Every planner available on this target, in display order.
pub fn entries() -> &'static [PlannerEntry] {
    crate::planners::REGISTRY
}

pub fn find(id: &str) -> Option<&'static PlannerEntry> {
    entries().iter().find(|entry| entry.info.id == id)
}

/// The planner `id` and `params` checked against its declarations, with
/// defaults filled in.
pub fn resolve(
    id: &str,
    params: &ParamValues,
) -> Result<(&'static PlannerEntry, ResolvedParams), PlannerError> {
    let entry = find(id).ok_or_else(|| PlannerError::UnknownPlanner { id: id.to_string() })?;
    let resolved = params::resolve(entry.info.params, params)
        .map_err(|error| PlannerError::Param { planner: entry.info.id, error })?;
    Ok((entry, resolved))
}

pub fn build(
    id: &str,
    params: &ParamValues,
    fleet: usize,
) -> Result<Box<dyn Planner>, PlannerError> {
    let (entry, resolved) = resolve(id, params)?;
    Ok((entry.build)(&resolved, fleet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::params::{ParamError, ParamValue, ParamValues};
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique() {
        let ids: HashSet<&str> = entries().iter().map(|e| e.info.id).collect();
        assert_eq!(ids.len(), entries().len());
    }

    #[test]
    fn default_planner_is_registered() {
        assert!(find(crate::planners::DEFAULT).is_some());
    }

    #[test]
    fn every_planner_builds_with_default_params() {
        for entry in entries() {
            assert!(build(entry.info.id, &ParamValues::default(), 12).is_ok(), "{}", entry.info.id);
        }
    }

    #[test]
    fn unknown_planner_is_an_error() {
        assert_eq!(
            resolve("no_such_planner", &ParamValues::default()).err(),
            Some(PlannerError::UnknownPlanner { id: "no_such_planner".into() })
        );
    }

    #[test]
    fn undeclared_param_is_an_error() {
        let values = ParamValues::default().with("undeclared_key", ParamValue::Int(1));
        for entry in entries() {
            assert_eq!(
                resolve(entry.info.id, &values).err(),
                Some(PlannerError::Param {
                    planner: entry.info.id,
                    error: ParamError::Unknown { key: "undeclared_key".into() },
                })
            );
        }
    }

    #[test]
    fn declared_defaults_lie_within_bounds() {
        for entry in entries() {
            assert!(resolve(entry.info.id, &ParamValues::default()).is_ok(), "{}", entry.info.id);
        }
    }

    #[test]
    fn assigns_tasks_matches_task_scheduler() {
        for entry in entries() {
            let planner = build(entry.info.id, &ParamValues::default(), 12).unwrap();
            assert_eq!(
                planner.task_scheduler().is_some(),
                entry.info.assigns_tasks,
                "{}",
                entry.info.id
            );
        }
    }
}
