# Contributing to MAFIS

Thanks for your interest in contributing! Here's how to get started.

## Reporting Bugs

Open a [bug report](https://github.com/stasis-industries/mafis/issues/new?template=bug_report.yml) with:

- Steps to reproduce
- Expected vs actual behavior
- Browser / OS / environment details

## Suggesting Features

Open a [feature request](https://github.com/stasis-industries/mafis/issues/new?template=feature_request.yml) describing the motivation and any alternatives you've considered.

## Development Setup

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli basic-http-server

# Fast feedback loop (no WASM build needed for logic changes)
cargo check    # ~5s — type & borrow check
cargo test     # ~10s — 500+ tests

# Full WASM build (only needed for rendering/bridge/ECS changes)
sh topologies/build-manifest.sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --out-dir web --target web \
  target/wasm32-unknown-unknown/release/mafis.wasm
basic-http-server web   # localhost:4000
```

## Adding a planner

A planner lives in one folder, `src/planners/<name>/`:

1. Implement `Planner` (`src/planning/planner.rs`). Each tick, `step()` gets
   an `Observation` (tick, grid, zones, agents) and returns `Decision::Plans`
   or `Decision::Keep`. Optional hooks: `snapshot`/`restore` for rewind,
   `telemetry` for numbers exported as `telemetry.<key>` columns, and
   `task_scheduler` for a planner that assigns its own tasks.
2. In the folder's `mod.rs`, export `ENTRY: PlannerEntry`: its `PlannerInfo`
   (id, label, summary, reference, declared parameters, whether it assigns
   tasks), a `build` function that receives the checked parameters and the
   fleet size, a `fleet_limit` (`no_fleet_limit` if there is none), and its
   `type_name`.
3. Add `<name>::ENTRY` to `REGISTRY` in `src/planners/mod.rs`. A planner that
   cannot run in the browser goes behind `#[cfg(not(target_arch = "wasm32"))]`,
   as RHCR does.

The web app, the desktop app and the experiment runner then offer it, and the
shared tests cover it: the tests that loop over the registry, and the golden
runs (bless its new lines with `MAFIS_BLESS_GOLDEN=1 cargo test --test
golden_runs`). Tests of its own behaviour go in its folder;
`tests/naming_guard.rs` fails if code outside `src/planners/` names it. Shared
building blocks (A*, the PIBT core, distance maps) are in
`src/planners/toolkit/` and `src/planning/heuristics.rs`.

## Pull Requests

1. Fork the repo and branch from `main`
2. Make sure `cargo check` and `cargo test` pass
3. If you changed rendering, bridge, or ECS systems, verify the WASM build works
4. Open a PR against `main` with a clear description of the change

## Code Style

- Standard Rust formatting (`cargo fmt`)
- No magic numbers — tunable limits go in `src/constants.rs`
- Determinism matters — all randomness must go through `SeededRng`

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you agree to uphold it.
