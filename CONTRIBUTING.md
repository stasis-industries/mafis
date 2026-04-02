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

## Pull Requests

1. Fork the repo and branch from `develop`
2. Make sure `cargo check` and `cargo test` pass
3. If you changed rendering, bridge, or ECS systems, verify the WASM build works
4. Open a PR against `develop` with a clear description of the change

## Code Style

- Standard Rust formatting (`cargo fmt`)
- No magic numbers — tunable limits go in `src/constants.rs`
- Determinism matters — all randomness must go through `SeededRng`

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By participating, you agree to uphold it.
