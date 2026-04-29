pub mod solver;
#[cfg(any(target_arch = "wasm32", not(feature = "headless")))]
pub(crate) use solver::PibtSolver;
pub use solver::{PibtLifelongSolver, default_active_solver};
