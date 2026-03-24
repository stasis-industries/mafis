# MAFIS — Multi-Agent Fault Injection Simulator

<!-- TODO: Replace with actual hero GIF showing fault cascade → recovery -->
<!-- Record: OBS, 1200x600, 15s, agents running → burst kills 30% → cascade → recovery visible in charts -->
<!-- ![MAFIS fault cascade demo](docs/hero.gif) -->

A fault resilience observatory for lifelong multi-agent pathfinding — 30K lines of Rust, running in your browser.

**[Live Demo](https://stasis-website.vercel.app/simulator)** | **Solo project by [Teddy Truong](https://github.com/teddytruong)**

`5 solvers` | `397 tests` | `30K LOC Rust` | `8,520 experiments` | `WASM 3D` | `Deterministic replay` | `Shareable URLs`

---

## Key Findings

**1. Braess's paradox in fault-injected MAPF.** Under congestion, killing agents paradoxically *improves* throughput for reactive solvers — PIBT gains +32% when agents die at 40-agent density, because dead agents free corridors. The effect is architecture-dependent: Token Passing is the only solver where permanent failures *hurt* (−42% throughput), because it depends on fleet completeness for sequential planning.

**2. The Braess threshold correlates with solver coordination depth.** Reactive solvers (PIBT) are always in Braess territory. Coordinated solvers (Token Passing) resist the paradox until extreme density (n=80). This was measured across 6,000 runs (5 solvers × 4 densities × 6 fault scenarios × 50 seeds).

| Solver | Braess threshold | Permanent/Recoverable ratio at n=40 |
|--------|-----------------|--------------------------------------|
| PIBT | n=10 (always) | 1.32x (helped by deaths) |
| RHCR variants | n=20-40 | 1.09-1.25x |
| Token Passing | n=80 (extreme only) | 0.58x (hurt by deaths) |

**3. Scheduler choice has only ~10% effect** — the solver algorithm and fault type dominate.

Prior work ([Hoenig et al. 2019](https://whoenig.github.io/publications/2019_RA-L_Hoenig.pdf), [Li et al. 2024](https://arxiv.org/abs/2404.16162)) addresses **delay robustness** (temporary slowdowns). To our knowledge, no prior work measures lifelong MAPF solver throughput under **permanent fault injection** — crash failures, Weibull-modeled wear, and zone outages.

<!-- TODO: Add "Open in Observatory" link once shareable URL is generated -->

---

## What It Does

MAFIS runs lifelong MAPF simulations in 3D, injects faults (crash failures, mechanical wear, zone outages, intermittent glitches), and measures how the system degrades and recovers — compared against a deterministic fault-free baseline.

Every simulation is seeded and reproducible. Every metric is differential: faulted vs baseline, same seed, same agents.

---

## Architecture

```
                           ┌─────────────────────────────┐
                           │      Bevy 0.18 ECS          │
                           │  FixedUpdate (deterministic) │
                           └──────────┬──────────────────┘
                                      │
              ┌───────────────────────┼───────────────────────┐
              ▼                       ▼                       ▼
    ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
    │   core/          │   │   solver/        │   │   fault/         │
    │   Tick loop      │   │   5 MAPF solvers │   │   Weibull wear   │
    │   Task scheduler │──▶│   A* + BFS       │   │   Burst/zone/    │
    │   Queue manager  │   │   heuristics     │   │   intermittent   │
    │   Agent FSM      │   └─────────────────┘   └─────────────────┘
    └────────┬────────┘
             │
    ┌────────▼────────┐   ┌─────────────────┐   ┌─────────────────┐
    │   analysis/      │   │   render/        │   │   ui/             │
    │   ADG + cascade  │   │   3D visuals     │   │   Bevy↔JS bridge  │
    │   Fault metrics  │   │   MaterialPalette│   │   wasm_bindgen    │
    │   Heatmap        │   │   Orbit camera   │   │   HTML/CSS/JS     │
    │   Baseline engine│   └─────────────────┘   │   uPlot charts    │
    └─────────────────┘                          └─────────────────┘
             │
    ┌────────▼────────┐
    │   experiment/    │   ← Headless: 2,520 runs, 30 seeds, 95% CIs
    │   Runner + stats │     CSV / JSON / LaTeX / Typst export
    │   Paper matrices │
    └─────────────────┘
```

```
src/
  core/        Tick loop, agents, grid, task scheduling, topology, delivery queues
  solver/      PIBT + 3 RHCR variants + Token Passing, shared A* and heuristics
  fault/       Weibull wear model, burst/zone/intermittent scenarios, fault schedule
  analysis/    ADG, cascade BFS, fault metrics, heatmap, resilience scorecard, baseline engine
  render/      3D environment, robot visuals (MaterialPalette), orbit camera
  ui/          Bevy-JS bridge (wasm-bindgen), HTML/CSS/JS controls, uPlot charts
  experiment/  Headless experiment runner, CSV/JSON/LaTeX/Typst export

topologies/    5 JSON warehouse layouts (Amazon, FedEx, Ocado inspired)
web/           HTML/CSS/JS shell, generated WASM artifacts
tests/         Integration tests: verification suite, paper experiments
```

---

## Solvers

All implemented from academic papers. No external solver crates.

| Solver | Paper | Characteristic |
|--------|-------|----------------|
| **PIBT** | Okumura 2022 | One-step reactive, O(n log n), lifelong-native |
| **RHCR (PBS)** | Li et al. 2021 | Windowed Priority-Based Search |
| **RHCR (PIBT-Window)** | Li et al. 2021 | Unrolled PIBT for H steps |
| **RHCR (Priority A*)** | Li et al. 2021 | Sequential spacetime A* |
| **Token Passing** | Ma et al. 2017 | Decentralized sequential via shared token |

---

## Fault Scenarios

| Scenario | Model | Effect |
|----------|-------|--------|
| **Burst Failure** | Kill X% of fleet at tick T | Sudden capacity loss |
| **Wear-Based** | Weibull inverse CDF per agent | Progressive mechanical degradation |
| **Zone Outage** | Latency on busiest zone | Temporary regional disruption |
| **Intermittent** | Exponential inter-arrival | Recurring temporary unavailability |

**Weibull presets** calibrated to published AGV reliability data:

| Level | Beta | Eta | MTTF | Source |
|-------|------|-----|------|--------|
| Low | 2.0 | 900 | ~798 | CASUN AGV (well-maintained) |
| Medium | 2.5 | 500 | ~444 | Canadian AGV survey |
| High | 3.5 | 150 | ~137 | Carlson & Murphy 2006 |

---

## Topologies

5 industry-inspired layouts:

| Topology | Size | Model |
|----------|------|-------|
| Warehouse Small | 22x9 | Amazon Kiva (small FC) |
| Warehouse Medium | 32x21 | Amazon Kiva (standard FC) |
| Kiva Large | 57x33 | Amazon Robotics (large FC) |
| Sorting Center | 40x20 | FedEx/UPS (3 chokepoints) |
| Compact Grid | 24x24 | Ocado micro-fulfillment |

---

## Metrics

### Differential (faulted vs baseline)

| Metric | Definition |
|--------|-----------|
| **Fault Tolerance** | `P_fault / P_nominal` — fraction of baseline throughput retained |
| **Throughput Recovery** | Ticks until per-tick throughput returns to baseline rate |
| **Deficit Recovery** | Ticks until cumulative task deficit closes |
| **NRR** | `1 - throughput_recovery/MTBF` — rate-based Normalized Recovery Ratio |
| **Critical Time** | Fraction of post-fault ticks below 50% baseline throughput |
| **Impacted Area** | Cumulative task deficit as % of baseline |

### Per-Agent / Per-Event

| Metric | Definition |
|--------|-----------|
| **Survival Rate** | Alive agents / initial fleet at simulation end |
| **Propagation Rate** | Avg fraction of fleet affected per fault event |
| **Wait Ratio** | Living agent-ticks spent waiting / total living agent-ticks |
| **MTBF** | Mean ticks between fault events |

---

## Experiment Infrastructure

```bash
# Paper matrices (~3.5 min)
cargo test --release --test paper_experiments full_paper_matrix -- --ignored --nocapture

# Braess resilience (~30 min)
cargo test --release --test paper_experiments braess_resilience -- --ignored --nocapture
```

8 experiment matrices, 30-50 seeds, 95% confidence intervals:

| Experiment | Variables | Runs | Seeds |
|-----------|-----------|------|-------|
| Solver resilience | 4 solvers x 6 scenarios | 720 | 30 |
| Topology effect | 4 topologies x 6 scenarios | 720 | 30 |
| Scale sensitivity | 4 fleet sizes x 6 scenarios | 720 | 30 |
| Scheduler effect | 2 schedulers x 6 scenarios | 360 | 30 |
| **Braess resilience** | **5 solvers x 4 densities x 6 scenarios** | **6,000** | **50** |

**Engineering audit:** 24 verification tests — collision-free guarantees, metrics formulas, determinism, RNG isolation, Weibull cross-validation, CI95 matching.

---

## Quick Start

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli basic-http-server

# Build
sh topologies/build-manifest.sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --out-dir web --target web \
  target/wasm32-unknown-unknown/release/mafis.wasm
basic-http-server web   # → http://localhost:4000
```

**Fast feedback (no WASM build):**

```bash
cargo check   # types + borrows  (~5s)
cargo test    # 396 tests        (~10s)
```

---

## License

MIT
