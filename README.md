# MAFIS — Multi-Agent Fault Injection Simulator

A fault resilience observatory for lifelong multi-agent pathfinding — 29K lines of Rust, running in your browser.

**Solo project by Teddy Music — UQAC / ESGI, 2025-2026**

`5 solvers` `5 topologies` `4 fault scenarios` `364 tests` `29K LOC Rust` `WASM 3D` `Deterministic replay`

**[Live Simulator](https://stasisai.dev/simulator) &middot; [Results](#findings) &middot; [GitHub](https://github.com/stasis-industry/mafis)**

---

## What It Does

MAFIS runs lifelong MAPF simulations in 3D, injects faults (crash failures, mechanical wear, zone outages, intermittent glitches), and measures how the system degrades and recovers — compared against a deterministic fault-free baseline.

Every simulation is seeded and reproducible. Every metric is differential: faulted vs baseline, same seed, same agents.

---

## Findings

2,700 experiment runs (30 seeds x 4 experiments), all metrics audited for scientific reliability.

### Task scheduling impacts fault recovery 3.8x more than solver choice

Closest-first scheduling completes **3.8x more tasks** than Random under a 20% burst failure (29.6 vs 7.8 tasks, n=30, p<0.001). The scheduling strategy — not the pathfinding algorithm — determines how quickly a warehouse recovers from faults.

### High-throughput solvers degrade hardest

Token Passing produces **9x more tasks** than PIBT in normal operation (70.2 vs 7.8). But under sustained wear (Weibull, 90% fleet dead by tick 500), Token Passing's fault tolerance drops to **FT=0.14** — it retains only 14% of baseline throughput. PIBT retains 100%. More capacity means more to lose.

### Dense layouts exhibit Braess's paradox

In the compact grid (Ocado-style micro-fulfillment), killing 20% of agents **improves throughput 8x** (FT=8.10). Removing agents relieves congestion bottlenecks — the classic Braess's paradox, observed in multi-agent systems.

---

## Solvers

All solvers implemented from academic papers. No external solver crates.

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

## Quick Start

```bash
# Prerequisites
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli basic-http-server

# Build topology manifest
sh topologies/build-manifest.sh

# Compile + bind + serve
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --out-dir web --target web \
  target/wasm32-unknown-unknown/release/mafis.wasm
basic-http-server web   # → http://localhost:4000
```

**Fast feedback (no WASM build):**

```bash
cargo check   # types + borrows  (~5s)
cargo test    # 364 tests        (~7s)
```

---

## Architecture

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

## Metrics

### Differential (faulted vs baseline)

| Metric | Definition |
|--------|-----------|
| **Fault Tolerance** | `P_fault / P_nominal` — fraction of baseline throughput retained |
| **Throughput Recovery** | Ticks until per-tick throughput returns to baseline rate |
| **Deficit Recovery** | Ticks until cumulative task deficit closes |
| **NRR** | `1 - recovery/MTBF` — Normalized Recovery Ratio (Or 2025) |
| **Critical Time** | Fraction of post-fault ticks below 50% baseline throughput |
| **Impacted Area** | Cumulative task deficit as % of baseline |

### Per-Agent / Per-Event

| Metric | Definition |
|--------|-----------|
| **Survival Rate** | Alive agents / initial fleet at simulation end |
| **Propagation Rate** | Avg fraction of fleet affected per fault event |
| **Wait Ratio** | Living agent-ticks spent waiting / total living agent-ticks |
| **MTBF** | Mean ticks between fault events (Or 2025) |

---

## Experiment Infrastructure

```bash
# Run the full 2,700-run matrix (~12 min, release mode)
cargo test --release --test paper_experiments full_paper_matrix -- --ignored --nocapture

# Results written to results/ as CSV + JSON + LaTeX + Typst
```

4 experiments, 30 seeds each, 95% confidence intervals:
1. **Solver resilience** — 4 solvers x 6 fault scenarios
2. **Topology effect** — 5 topologies x 6 scenarios
3. **Scale sensitivity** — 4 fleet sizes x 6 scenarios
4. **Scheduler effect** — 2 schedulers x 6 scenarios

---

## License

MIT
