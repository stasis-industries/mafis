# MAFIS — Multi-Agent Fault Injection Simulator

A fault resilience observatory for lifelong multi-agent pathfinding — 30K lines of Rust, running in your browser.

**[Live Demo](https://stasis-website.vercel.app/simulator)** | **Solo project by [Teddy Truong](https://github.com/teddytruong)**

`5 solvers` | `408 tests` | `30K LOC Rust` | `7,000 experiments` | `WASM 3D` | `Deterministic replay` | `Shareable URLs`

---

## Key Findings

**1. Braess's paradox in fault-injected MAPF.** Under congestion, killing agents paradoxically *improves* throughput for reactive solvers — dead agents free corridors. The effect is architecture-dependent and confirms at 95% confidence across 7,000 runs (5 solvers × 4 densities × 7 scenarios × 50 seeds).

| Solver | Braess threshold | Ratio at n=40, burst 20% | p-value |
|--------|-----------------|--------------------------|---------|
| PIBT | **n=10** (always) | 1.586 [1.375, 1.797] | 0.008 |
| RHCR-PIBT | **n=10** | 1.587 [1.250, 1.923] | 0.009 |
| RHCR-PBS | **n=20** | 1.300 [1.159, 1.441] | 0.147 |
| RHCR-A* | **n=40** | 1.549 [1.363, 1.735] | <0.001 |
| Token Passing | **none** | 0.748 [0.673, 0.823] | <0.001 |

*95% CI lower bound > 1.0 = confirmed Braess benefit. Mann-Whitney U test.*

**The Braess threshold correlates with solver coordination depth.** Reactive solvers congested at any density; coordinated solvers resist the paradox longer.

**2. Token Passing is uniquely vulnerable to permanent faults.** The only solver with no confirmed Braess benefit — and worst-in-class under all permanent-fault scenarios:

| Scenario | TP ratio at n=10 | TP ratio at n=40 | Interpretation |
|----------|-----------------|-----------------|----------------|
| Burst 50% | 0.445 (p<0.001) | 0.546 (p<0.001) | Fleet gaps break sequential TOKEN |
| Wear (high) | 0.263 (p<0.001) | 0.590 (p<0.001) | Progressive death cascade |
| Perm. Zone | **0.178** (p<0.001) | 0.371 (p<0.001) | Worst scenario — space loss collapses TOKEN |

Actionable implication: Token Passing should not be deployed in environments with high permanent-fault risk at low fleet density.

**3. Permanent-localized faults (zone outage) show density-dependent vulnerability not seen in other fault types.** Blocking a zone permanently is catastrophic at low density (lost space fraction is large) but neutral or beneficial at high density (zone removal relieves corridor competition — a second Braess mechanism):

| Solver | Perm. Zone at n=10 | Perm. Zone at n=80 |
|--------|-------------------|-------------------|
| PIBT | 0.785 (p=0.011) | **1.000** (p=0.997) |
| RHCR-PIBT | 0.531 (p<0.001) | 0.963 (p=0.717) |
| RHCR-A* | 0.503 (p<0.001) | **1.057** (p=0.895) |

**4. Scheduler choice has only ~10% effect** — solver algorithm and fault type dominate.

Prior work ([Hoenig et al. 2019](https://whoenig.github.io/publications/2019_RA-L_Hoenig.pdf), [Li et al. 2024](https://arxiv.org/abs/2404.16162)) addresses **delay robustness** (temporary slowdowns). To our knowledge, no prior work measures lifelong MAPF solver throughput under **permanent fault injection** — crash failures, Weibull-modeled wear, and permanent zone loss.

---

## What It Does

MAFIS runs lifelong MAPF simulations in 3D, injects faults across three categories, and measures how the system degrades and recovers — compared against a deterministic fault-free baseline run in parallel.

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
    │   Queue manager  │   │   heuristics     │   │   intermittent/  │
    │   Agent FSM      │   └─────────────────┘   │   perm_zone      │
    └────────┬────────┘                          └─────────────────┘
             │
    ┌────────▼────────┐   ┌─────────────────┐   ┌─────────────────┐
    │   analysis/      │   │   render/        │   │   ui/             │
    │   ADG + cascade  │   │   3D visuals     │   │   Bevy↔JS bridge  │
    │   Fault metrics  │   │   MaterialPalette│   │   wasm_bindgen    │
    │   Heatmap        │   │   Orbit camera   │   │   HTML/CSS/JS     │
    │   Baseline engine│   └─────────────────┘   │   uPlot charts    │
    └────────┬────────┘                          └─────────────────┘
             │
    ┌────────▼────────┐
    │   experiment/    │   ← Headless: 7,000 runs, 50 seeds, 95% CIs
    │   Runner + stats │     CSV / JSON / LaTeX / Typst export
    │   Paper matrices │
    └─────────────────┘
```

```
src/
  core/        Tick loop, agents, grid, task scheduling, topology, delivery queues
  solver/      PIBT + 3 RHCR variants + Token Passing, shared A* and heuristics
  fault/       Weibull wear model, 3-category fault system, fault schedule
  analysis/    ADG, cascade BFS, fault metrics, heatmap, resilience scorecard, baseline engine
  render/      3D environment, robot visuals (MaterialPalette), orbit camera
  ui/          Bevy-JS bridge (wasm-bindgen), HTML/CSS/JS controls, uPlot charts
  experiment/  Headless experiment runner, CSV/JSON/LaTeX/Typst export

topologies/    5 JSON warehouse layouts (Amazon, FedEx, Ocado inspired)
web/           HTML/CSS/JS shell, generated WASM artifacts
analysis/      Python analysis scripts (Braess ratios, significance tests, charts)
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

## Fault Taxonomy

Three categories, seven scenarios:

| Category | Scenario | Model | Effect |
|----------|----------|-------|--------|
| **Recoverable** | Zone Outage | Latency on busiest zone | Temporary regional disruption |
| **Recoverable** | Intermittent | Exponential inter-arrival | Recurring temporary unavailability |
| **Permanent-distributed** | Burst Failure | Kill X% of fleet at tick T | Sudden capacity loss |
| **Permanent-distributed** | Wear-Based | Weibull inverse CDF per agent | Progressive mechanical degradation |
| **Permanent-localized** | Perm. Zone Outage | Zone cells → permanent obstacles | Zone flooding / structural collapse |

**Weibull presets** calibrated to published AGV reliability data:

| Level | Beta | Eta | MTTF | Source |
|-------|------|-----|------|--------|
| Low | 2.0 | 900 | ~798 ticks | CASUN AGV (well-maintained) |
| Medium | 2.5 | 500 | ~444 ticks | Canadian AGV survey |
| High | 3.5 | 150 | ~137 ticks | Carlson & Murphy 2006 |

---

## Topologies

5 industry-inspired layouts:

| Topology | Size | Model |
|----------|------|-------|
| Warehouse Small | 22×9 | Amazon Kiva (small FC) |
| Warehouse Medium | 32×21 | Amazon Kiva (standard FC) |
| Kiva Large | 57×33 | Amazon Robotics (large FC) |
| Sorting Center | 40×20 | FedEx/UPS (3 chokepoints) |
| Compact Grid | 24×24 | Ocado micro-fulfillment |

---

## Metrics

### Differential (faulted vs baseline, same seed)

| Metric | Definition |
|--------|-----------|
| **Fault Tolerance** | `P_fault / P_nominal` — fraction of baseline throughput retained |
| **Throughput Recovery** | Ticks until per-tick throughput returns to baseline rate |
| **Deficit Recovery** | Ticks until cumulative task deficit closes |
| **NRR** | `1 - throughput_recovery/MTBF` — Normalized Recovery Ratio |
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
# Run all paper experiments (~20 min, results/ output)
cargo test run_braess_perm_zone -- --ignored --nocapture

# Braess analysis: ratios, Mann-Whitney U, LaTeX table, SVG charts
python3 analysis/braess_analysis.py
```

| Experiment | Variables | Runs | Seeds |
|-----------|-----------|------|-------|
| Solver resilience | 4 solvers × 7 scenarios | 840 | 30 |
| Topology effect | 4 topologies × 7 scenarios | 840 | 30 |
| Scale sensitivity | 4 fleet sizes × 7 scenarios | 840 | 30 |
| Scheduler effect | 2 schedulers × 7 scenarios | 420 | 30 |
| **Braess resilience** | **5 solvers × 4 densities × 7 scenarios** | **7,000** | **50** |

**Engineering audit:** 408 tests — collision-free guarantees, metrics formulas, determinism, RNG isolation, Weibull cross-validation, CI95 matching, fault scenario roundtrips.

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
cargo test    # 408 tests        (~20s)
```

---

## License

MIT
