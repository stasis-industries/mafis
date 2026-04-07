//! LaCAM3 — engineered LaCAM* configuration-space MAPF solver.
//!
//! REFERENCE: docs/papers_codes/lacam3/ (Kei18/lacam3, AAMAS 2024)
//! Paper: Okumura — "Engineering LaCAM*: Towards Real-Time, Large-Scale, and
//! Near-Optimal Multi-Agent Pathfinding", AAMAS 2024.
//!
//! This is the **actual SOTA** single-shot MAPF solver as of late 2024,
//! wrapped in a lifelong replan loop to fit MAFIS's `LifelongSolver` trait.
//!
//! ## Module organization
//!
//! Files map directly to the C++ reference's `lacam3/src/` layout:
//!
//! | MAFIS file | C++ reference |
//! |-----------|---------------|
//! | `instance.rs` | `instance.cpp` + `graph.cpp` (foundation types) |
//! | `dist_table.rs` | `dist_table.cpp` (per-agent BFS distance table) |
//! | `lnode.rs` | `lnode.cpp` (low-level constraint propagation node) |
//! | `hnode.rs` | `hnode.cpp` (high-level configuration node) |
//! | `collision_table.rs` | `collision_table.cpp` (path collision tracking) |
//! | `pibt.rs` | `pibt.cpp` (lacam3's specialized PIBT — NOT shared with src/solver/shared/pibt_core.rs) |
//! | `scatter.rs` | `scatter.cpp` (SUO heuristic) |
//! | `planner.rs` | `planner.cpp` (LaCAM* main search loop) |
//! | `solver.rs` | NEW — lifelong wrapper implementing LifelongSolver |
//!
//! ## Adaptations to MAFIS architecture
//!
//! 1. **No graph data structure** — lacam3 builds a `Graph` with `Vertex*` neighbors.
//!    MAFIS uses `IVec2` cell coordinates with neighbors computed on-the-fly via
//!    `GridMap::walkable_neighbors`. We adopt the MAFIS approach: vertices are
//!    represented as flat `usize` indices (`y * width + x`) and the grid is
//!    queried directly.
//! 2. **No multi-threading** — lacam3 uses `std::async` for the configuration
//!    generator (`PIBT_NUM` workers in parallel) and refiner pool. WASM
//!    compilation precludes threading; MAFIS runs the configuration generator
//!    sequentially. This is documented as a deviation impacting search speed
//!    but not solution quality.
//! 3. **No refiner** — the refiner module is post-processing that improves
//!    initial solutions. Skipped in v1 (deferred to v2 if validation requires).
//! 4. **No SIPP** — only used by the refiner (which is skipped). Deferred.
//! 5. **Lifelong wrapper** — lacam3 is one-shot. The wrapper in `solver.rs`
//!    caches a plan and replans on (a) goal change, (b) fault event, (c) plan
//!    exhaustion, (d) every K ticks failsafe.

pub mod collision_table;
pub mod dist_table;
pub mod hnode;
pub mod instance;
pub mod lnode;
pub mod pibt;
pub mod planner;
pub mod scatter;
pub mod solver;

pub use solver::LaCAM3LifelongSolver;
