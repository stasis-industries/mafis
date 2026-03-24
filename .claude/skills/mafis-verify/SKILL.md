---
name: mafis-verify
description: >
  Full verification pipeline for MAFIS. Use this skill whenever the user says "verify",
  "check", "test", "does it compile", "does it work", "run tests", "build it", "try it",
  "make sure it works", "cargo check", "cargo test", "cargo run", "is it broken", "run it",
  "did I break anything", or any variation of wanting to validate their changes work. Also
  trigger when the user finishes writing code and wants to confirm correctness, or after any
  refactor, bugfix, or new feature. Trigger proactively after completing a coding task — don't
  wait for the user to ask. This skill knows when a WASM build is needed vs cargo test alone
  vs desktop cargo run.
---

# MAFIS Verification Pipeline

You are verifying the MAFIS project — a Bevy 0.18 simulator with two targets:
- **WASM** (web browser) — bridge.rs + HTML/CSS/JS
- **Native desktop** (egui) — `cargo run`

## Decision Tree

```
Was code modified?
├── No → nothing to verify
├── Yes → Step 1: cargo check (~5s)
│   ├── Fails → fix errors, re-check
│   └── Passes → Step 2: cargo test (~7s)
│       ├── Fails → fix errors, re-test
│       └── Passes (396 tests) → Does change touch rendering, bridge, ECS, or web/?
│           ├── No → DONE (report pass)
│           ├── Yes (web/bridge only) → Step 3a: WASM build
│           ├── Yes (desktop/egui only) → Step 3b: cargo run
│           └── Yes (render/ECS shared) → Step 3a + 3b
```

## Step 1 — Type Check (~5s)

```bash
cargo check
```

Catches types, borrows, imports, lifetimes. Fix before proceeding.

## Step 2 — Logic Tests (~7s)

```bash
cargo test
```

394 tests across 36 files: core (grid, actions, task scheduling, queue, topology), solver (PIBT, RHCR x3, Token Passing, A*, heuristics), analysis (cascade, ADG, fault_metrics, heatmap, scorecard, baseline, history), fault (scenarios, breakdown, manual), experiment (stats, runner, export, paper), and 46 SimHarness integration tests.

If tests fail: read the failing test, understand what it asserts, fix the code.
If test count drops below 394: flag it — a test may have been accidentally deleted.

## Step 3a — WASM Build (conditional, ~2-3 min)

```bash
sh web/topologies/build-manifest.sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --out-dir web --target web target/wasm32-unknown-unknown/release/mafis.wasm
```

**CRITICAL**: Use the **binary** target (hyphens: `mafis.wasm`), NOT cdylib (underscores). The cdylib compiles but the Bevy event loop never starts.

### WASM needed when:
- `src/ui/bridge.rs` changes (Bevy↔JS sync)
- `web/` changes (app.js, index.html)
- `src/render/` changes (visuals, camera, picking, heatmap)
- Bevy system ordering changes (`add_systems`, `.chain()`, `.after()`)
- New ECS components/resources used in render systems

### WASM NOT needed when:
- Pure logic changes (solver tweaks, analysis math, constants)
- Test-only changes
- Fault config, task scheduling, topology generation
- Desktop UI changes (`src/ui/desktop/`)
- Anything `cargo test` fully covers

## Step 3b — Desktop Build (conditional)

```bash
cargo run
```

Desktop needed when `src/ui/desktop/` changes (panels, toolbar, timeline, charts, theme, shortcuts). Not needed for web-only or pure logic changes.

## Post-Build

WASM: check if dev server is running:
```bash
lsof -i :4000 | grep LISTEN
```
If not: `basic-http-server web` (port 4000).

## Quick Check Mode

If user says "quick check" or "does it compile": run only `cargo check`, report pass/fail.

## Reporting

Be concise:
- **Pass**: "cargo check passed. 394 tests passed. No WASM build needed." (one line)
- **Fail**: Show first error, identify file:line, suggest fix.
- **WASM**: Report build success/failure and whether server is running.
