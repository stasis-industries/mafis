<div align="center">
  <img src="assets/logo/logo.svg" width="400" alt="MAFIS">
  <br><br>

  [![CI](https://img.shields.io/github/actions/workflow/status/stasis-industries/mafis/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/stasis-industries/mafis/actions/workflows/ci.yml)
  [![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
  [![Rust](https://img.shields.io/badge/rust-2024-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
  [![WASM](https://img.shields.io/badge/target-wasm32-654ff0?style=flat-square&logo=webassembly)](https://webassembly.org/)
  [![Bevy](https://img.shields.io/badge/bevy-0.18-232326?style=flat-square)](https://bevyengine.org/)
  [![Demo](https://img.shields.io/badge/demo-live-brightgreen?style=flat-square)](https://stasis-website.vercel.app/simulator)

  <br>
  <p><em>A fault resilience observatory for lifelong multi-agent pathfinding.<br>
  Inject faults. Observe degradation. Measure recovery.</em></p>

  **[Live Demo](https://stasis-website.vercel.app/simulator)** · **[Docs](https://stasis-website.vercel.app/docs)** · **Solo project by [Teddy Truong](https://github.com/onsraa)**
</div>

---

Most MAPF tools measure how fast solvers find paths. **MAFIS** (Multi-Agent Fault Injection Simulator) measures what happens when things go *wrong* — pairing every faulted run with a fault-free baseline so every metric is a deviation, not an absolute number. It is not a solver benchmark.

---

## Quick Start

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
cargo check   # ~5s
cargo test    # ~3 min, 502 tests
```

---

## Roadmap

See [ROADMAP.md](ROADMAP.md) for planned improvements and known gaps.

## Contributing & quality gates

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to report bugs, run tests, and submit pull requests. Quality gates for adding solvers, fault models, metrics, or topologies are tracked in [RELIABILITY.md](RELIABILITY.md).

## Security

To report a vulnerability, see [SECURITY.md](SECURITY.md).

---

## Acknowledgments

MAFIS implements algorithms from published research:

- **Okumura et al.** — *Priority Inheritance with Backtracking for Iterative Multi-agent Path Finding*, [arXiv:1901.11282](https://arxiv.org/abs/1901.11282), AAAI 2019
- **Li et al.** — *Lifelong Multi-Agent Path Finding in Large-Scale Warehouses*, [arXiv:2005.07371](https://arxiv.org/abs/2005.07371), AAAI 2021
- **Ma et al.** — *Lifelong Multi-Agent Path Finding for Online Pickup and Delivery Tasks*, [arXiv:1705.10868](https://arxiv.org/abs/1705.10868), AAMAS 2017

---

MIT
