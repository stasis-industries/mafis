# MAFIS — Multi-Agent Fault Injection Simulator

Most MAPF tools measure how fast solvers find paths. MAFIS measures what happens when things go *wrong*.

A fault resilience observatory for lifelong multi-agent pathfinding — 30K lines of Rust, running in your browser.

**[Live Demo](https://stasis-website.vercel.app/simulator)** | **[Docs](https://stasis-website.vercel.app/docs)** | **Solo project by [Teddy Truong](https://github.com/onsraa)**

---

## What it does

MAFIS runs two simulations in parallel: a fault-free *baseline* and a *faulted* run with the same seed, same agents, same grid. Every metric is a deviation from that baseline — so when throughput drops, you know how much, and you know it was the fault that caused it.

Inject faults. Observe degradation. Measure recovery. Compare solvers.

The simulation is deterministic. Same seed, same config, same results. Every number is reproducible.

---

## The instrument

### Solvers

Eight lifelong solvers spanning five paradigms, implemented from the literature. No external solver crates.

| Solver | Reference | Paradigm |
|--------|-----------|----------|
| PIBT | Okumura 2022 | Reactive |
| RHCR (PBS) | Li et al. 2021 | Windowed |
| RHCR (PIBT-Window) | Li et al. 2021 | Windowed |
| RHCR (Priority A*) | Li et al. 2021 | Windowed |
| Token Passing | Ma et al. 2017 | Decentralized |
| RT-LaCAM | Liang et al. 2025 | Config-space |
| TPTS | Ma et al. 2017 | Decentralized |
| PIBT+APF | Pertzovsky et al. 2025 | Meta + Reactive |

### Fault taxonomy

Three categories of faults, modeled after real-world failure modes.

| Category | Scenario | What it models |
|----------|----------|----------------|
| *Recoverable* | Zone outage | Sensor network failure, temporary zone lockdown |
| *Recoverable* | Intermittent | Battery reconnect, sensor recalibration |
| *Permanent-distributed* | Burst failure | Power surge, software crash across fleet |
| *Permanent-distributed* | Wear-based | Mechanical degradation (Weibull-calibrated) |
| *Permanent-localized* | Perm. zone outage | Fire suppression, structural collapse, flooding |

Weibull presets are calibrated to published AGV reliability data — from well-maintained fleets (CASUN AGV, MTTF ~798 ticks) to high-stress deployments (Carlson & Murphy 2006, MTTF ~137 ticks).

### Topologies

Five industry-inspired warehouse layouts, defined as JSON.

| Topology | Size | Inspired by |
|----------|------|-------------|
| Warehouse Small | 22x9 | Amazon Kiva (small FC) |
| Warehouse Medium | 32x21 | Amazon Kiva (standard FC) |
| Kiva Large | 57x33 | Amazon Robotics (large FC) |
| Sorting Center | 40x20 | FedEx/UPS hub (3 chokepoints) |
| Compact Grid | 24x24 | Ocado micro-fulfillment |

### Resilience metrics

All metrics are *differential* — faulted run compared to the paired baseline.

| Metric | What it measures |
|--------|-----------------|
| Fault Tolerance | Fraction of baseline throughput retained under fault |
| NRR | Normalized Recovery Ratio — how quickly the system bounces back |
| Critical Time | Fraction of post-fault ticks below 50% baseline throughput |
| Impacted Area | Cumulative task deficit relative to baseline |
| Throughput Recovery | Ticks until per-tick throughput returns to baseline rate |
| Survival Rate | Fraction of fleet still operational at simulation end |

---

## The engine

Built with **Rust** and **Bevy 0.18 ECS**. Compiled to **WebAssembly** — no installation, runs in any modern browser.

- Deterministic ECS tick loop, independent of frame rate
- Instanced rendering for up to 500 agents
- Bevy-to-JS bridge via `wasm_bindgen` with adaptive polling
- Dual heatmaps (density decay + cumulative traffic)
- Orbit camera, click-to-select, 8-state task visualization
- Shareable URLs — configuration encodes to a URL fragment, no server required

---

## Experiment infrastructure

MAFIS includes a headless experiment runner for large-scale studies. No browser needed — Rayon-parallelized, outputs CSV, JSON, LaTeX, Typst, and SVG.

| Experiment | Design | Runs |
|-----------|--------|------|
| Solver resilience | 4 solvers x 7 scenarios x 30 seeds | 840 |
| Topology effect | 4 topologies x 7 scenarios x 30 seeds | 840 |
| Scale sensitivity | 4 fleet sizes x 7 scenarios x 30 seeds | 840 |
| Scheduler effect | 2 schedulers x 7 scenarios x 30 seeds | 420 |
| Braess resilience | 5 solvers x 4 densities x 7 scenarios x 50 seeds | 7,000 |
| Cross-topology | 2 solvers x 2 topologies x 2 scenarios x 2 densities x 30 seeds | 480 |

Statistical tooling: Benjamini-Hochberg FDR correction, Cliff's delta effect sizes, 95% CIs. Analysis script in pure Python (no dependencies).

Preliminary observations from these experiments are documented on the [blog](https://stasis-website.vercel.app/blog).

---

## Ecosystem

MAFIS answers one question: *how do multi-agent systems behave under faults?*

Everything else has a home:

- **[MAPF Tracker](https://tracker.pathfinding.ai/)** — solver benchmarks on clean instances
- **[MovingAI](https://movingai.com/benchmarks/)** — standard grid instances
- **[LSMART](https://smart-mapf.github.io/lifelong-smart)** — realistic FMS evaluation with kinodynamic execution

---

## Quick start

**Browser (no install):** [stasis-website.vercel.app/simulator](https://stasis-website.vercel.app/simulator)

**Build from source:**

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli basic-http-server

sh topologies/build-manifest.sh
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --out-dir web --target web \
  target/wasm32-unknown-unknown/release/mafis.wasm
basic-http-server web   # localhost:4000
```

**Fast feedback (no WASM build):**

```bash
cargo check   # types + borrows  (~5s)
cargo test    # 473 tests        (~3 min)
```

---

## Architecture

```
src/
  core/        Tick loop, agents, grid, task scheduling, topology, delivery queues
  solver/      8 solvers: PIBT, 3 RHCR, Token Passing, RT-LaCAM, TPTS, PIBT+APF
  fault/       Weibull wear model, 3-category fault system, fault schedule
  analysis/    ADG, cascade BFS, fault metrics, heatmap, resilience scorecard
  render/      3D environment, robot visuals (MaterialPalette), orbit camera
  ui/          Bevy-JS bridge (wasm-bindgen), HTML/CSS/JS controls, uPlot charts
  experiment/  Headless experiment runner, statistical export

topologies/    5 JSON warehouse layouts
web/           HTML/CSS/JS shell, generated WASM artifacts
analysis/      Python analysis scripts
```

---

MIT
