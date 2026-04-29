use bevy::prelude::*;
use std::fmt;

use crate::core::action::Action;
use crate::core::grid::GridMap;

// ---------------------------------------------------------------------------
// Solver metadata types
// ---------------------------------------------------------------------------

/// Whether the solver guarantees optimal solutions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Optimality {
    Optimal,
    Bounded,
    Suboptimal,
}

impl Optimality {
    pub fn label(&self) -> &'static str {
        match self {
            Optimality::Optimal => "Optimal",
            Optimality::Bounded => "Bounded",
            Optimality::Suboptimal => "Suboptimal",
        }
    }
}

/// How well the solver scales with agent count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalability {
    Low,
    Medium,
    High,
}

impl Scalability {
    pub fn label(&self) -> &'static str {
        match self {
            Scalability::Low => "Low",
            Scalability::Medium => "Medium",
            Scalability::High => "High",
        }
    }
}

/// Structured metadata describing a solver's characteristics.
pub struct SolverInfo {
    pub optimality: Optimality,
    pub complexity: &'static str,
    pub scalability: Scalability,
    pub description: &'static str,
    pub source: &'static str,
    pub recommended_max_agents: Option<usize>,
}

// ---------------------------------------------------------------------------
// Solver error + trait
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SolverError {
    NoSolution,
    Timeout,
    InvalidInput(String),
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolverError::NoSolution => write!(f, "no solution found"),
            SolverError::Timeout => write!(f, "solver timed out"),
            SolverError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
        }
    }
}

pub trait MAPFSolver: Send + Sync + 'static {
    /// Short identifier for this solver (e.g. `"pibt"`, `"rhcr_pbs"`, `"token_passing"`).
    fn name(&self) -> &str;

    /// Structured metadata about this solver's properties.
    fn info(&self) -> SolverInfo;

    fn solve(
        &self,
        grid: &GridMap,
        agents: &[(IVec2, IVec2)],
    ) -> Result<Vec<Vec<Action>>, SolverError>;
}
