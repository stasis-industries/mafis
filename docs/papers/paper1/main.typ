// ============================================================
// MAFIS — Tool Paper (Paper 1)
// "MAFIS: A Fault Resilience Observatory for Lifelong MAPF"
// Target: SoCS short paper / AAMAS demo / ICAPS systems track
// ============================================================

#set document(
  title: "MAFIS: A Fault Resilience Observatory for Lifelong Multi-Agent Path Finding",
  author: "Teddy Truong",
)
x
#set page(
  paper: "us-letter",
  margin: (x: 0.75in, y: 1in),
  numbering: "1",
  number-align: center,
)

#set text(font: "Linux Libertine", size: 10pt, lang: "en")
#set par(justify: true, leading: 0.55em)
#set heading(numbering: "1.")
#show heading.where(level: 1): it => {
  set text(size: 10.5pt, weight: "bold")
  v(0.8em, weak: true)
  it
  v(0.4em, weak: true)
}
#show heading.where(level: 2): it => {
  set text(size: 10pt, weight: "bold", style: "italic")
  v(0.5em, weak: true)
  it
  v(0.3em, weak: true)
}

// ── helpers ──────────────────────────────────────────────────
#let todo(msg) = text(fill: rgb("#c0392b"), weight: "bold", [TODO: #msg])

// ── Title block ──────────────────────────────────────────────
#align(center)[
  #text(size: 14pt, weight: "bold")[
    MAFIS: A Fault Resilience Observatory for Lifelong\
    Multi-Agent Path Finding
  ]
  #v(0.6em)
  #text(size: 11pt)[Teddy Truong]
  #v(0.2em)
  #text(size: 10pt, style: "italic")[
    Université du Québec à Chicoutimi (UQAC) /
    École Supérieure de Génie Informatique (ESGI)
  ]
  #v(0.2em)
  #text(size: 9pt)[#link("https://stasis-website.vercel.app/simulator")[Live demo] ·
  #link("https://github.com/stasis-industries/mafis")[github.com/stasis-industries/mafis]]
]

#v(1em)

// ── Abstract ─────────────────────────────────────────────────
#block(
  stroke: (left: 2pt + rgb("#2c3e50")),
  inset: (left: 8pt, rest: 0pt),
)[
  *Abstract.* We present MAFIS, a browser-based observatory for measuring
  fault resilience in lifelong multi-agent path finding (MAPF). Existing
  MAPF tools evaluate solver performance under clean operating conditions;
  no tool measures how lifelong solvers degrade, recover, and adapt when
  robots fail permanently, zones become impassable, or mechanical wear
  accumulates. MAFIS fills this gap with a three-category fault taxonomy,
  twelve differential resilience metrics computed against a deterministic
  baseline, and real-time 3D visualization — all running in-browser via
  WebAssembly with shareable URLs. Five lifelong solvers are implemented
  from the literature. To demonstrate the observatory's utility, we report
  an 8,480-run experiment (Benjamini-Hochberg corrected, $alpha = 0.05$)
  showing that fault-induced congestion relief is solver- and
  topology-dependent: burst failures improve throughput up to 2.1× for
  reactive solvers on corridor-based layouts but not on chokepoint
  layouts, while Token Passing collapses to 18% throughput under
  permanent zone failure (Cliff's $delta = -1.0$). These findings are undetectable without
  MAFIS's differential measurement infrastructure. MAFIS is open-source
  (MIT), 30K lines of Rust, and compiles to a 15 MB browser binary.
]

#v(0.8em)
#columns(2, gutter: 0.5em)[

// ─────────────────────────────────────────────────────────────
= Introduction

Multi-agent path finding (MAPF) research has focused on collision-free
planning under clean conditions @stern2019mapf @okumura2022priority
@li2021lifelong. When robots crash, zones become inaccessible, or wear
accumulates, the system must degrade gracefully. This recovery behavior
is a first-class concern in deployed warehouse automation, yet no
existing tool measures it systematically.

Prior work on execution robustness handles _delays_: temporary slowdowns
absorbed with buffering @hoenig2019persistent, $k$-robust plans
@atzmon2020robust, or planning failure policies @morag2023adapting. The
League of Robot Runners @jiang2024scaling evaluates lifelong solvers
under normal conditions and explicitly identifies fault tolerance as
future work. Recent work on traffic flow optimization @chen2024traffic
documents throughput saturation at high density. None of these inject
permanent failures or measure differential solver degradation.

MAFIS fills this gap. It is an empirical instrument — not a solver
benchmark — that observes, measures, and visualizes how lifelong MAPF
systems degrade under configurable fault injection. Its contributions:

#set enum(numbering: "(1)", tight: true)
+ A three-category fault taxonomy (recoverable, permanent-distributed,
  permanent-localized) with Weibull-calibrated wear models (§3).
+ Twelve differential resilience metrics computed against a paired
  fault-free baseline (§4).
+ A browser-based 3D observatory with deterministic replay, cascade
  visualization, and shareable URLs — no installation required (§2, §7).
+ A demonstration experiment revealing solver- and topology-dependent
  fault-induced congestion relief, with Benjamini-Hochberg corrected
  significance tests and Cliff's delta effect sizes (§6).


// ─────────────────────────────────────────────────────────────
= System Design

MAFIS is 30,000 lines of Rust using Bevy 0.18 ECS, compiled to
WebAssembly for browser execution.

== Deterministic ECS

The simulation runs in Bevy's `FixedUpdate` schedule at a fixed
timestep, independent of frame rate. All random events use a single
`SeededRng`; given the same seed, every simulation produces identical
output. This enables _paired comparison_: the fault-free baseline runs
with the same seed as the faulted simulation, so metric differences
are causally attributable to the fault.

== Bevy↔JS Bridge

A `thread_local` `RefCell` buffer bridges ECS and browser. A Bevy system
serializes state to JSON at an adaptive interval (90 ms for $<=$ 50
agents, 500 ms for 500) via `wasm-bindgen`. Commands flow in reverse:
JS writes to the bridge, Bevy drains each frame. Above 50 agents,
summaries replace per-agent snapshots.

== Shareable URLs

Configuration serializes to JSON, compresses via Deflate (pako), and
encodes as a URL fragment. No server required — the simulation is fully
reconstructible from the URL alone.


// ─────────────────────────────────────────────────────────────
= Fault Taxonomy

MAFIS organizes faults into three categories by permanence and spatial
extent (Table 1).

#figure(
  table(
    columns: (auto, auto, 1fr),
    stroke: 0.4pt,
    align: (left, left, left),
    inset: 5pt,
    table.header(
      [*Category*], [*Scenario*], [*Effect*],
    ),
    [Recoverable],
      [Zone outage], [Latency on busiest zone for $d$ ticks],
    [],
      [Intermittent], [Exponential inter-arrival, 15-tick recovery],
    [Perm.-distributed],
      [Burst failure], [Kill $k$% of fleet at tick $t$],
    [],
      [Wear-based], [Weibull $F(t) = 1-e^{-(t\/eta)^beta}$ per agent],
    [Perm.-localized],
      [Perm. zone outage], [Zone cells → permanent obstacles at tick $t$],
  ),
  caption: [Three-category fault taxonomy with five scenario types.],
) <tab-taxonomy>

Wear presets are calibrated to published AGV reliability: Low ($beta=2.0,
eta=900$, MTTF $approx$ 798 ticks — CASUN AGV), Medium ($beta=2.5,
eta=500$, MTTF $approx$ 444 — Canadian AGV survey), High ($beta=3.5,
eta=150$, MTTF $approx$ 137 — Carlson & Murphy 2006).

A `FaultList` compiles multiple entries into a unified schedule; wear and
permanent-zone faults are limited to one per simulation to prevent
trivial collapse.


// ─────────────────────────────────────────────────────────────
= Resilience Metrics

All metrics are _differential_: comparing the faulted run to a
fault-free baseline with the same seed. This paired design eliminates
initialization variance.

#figure(
  table(
    columns: (auto, 1fr),
    stroke: 0.4pt,
    align: (left, left),
    inset: 5pt,
    table.header([*Metric*], [*Definition*]),
    [Fault Tolerance],
      [$P_"fault" \/ P_"nominal"$ — throughput retained vs.~baseline],
    [Throughput Recovery],
      [Ticks until per-tick throughput returns to baseline rate],
    [Deficit Recovery],
      [Ticks until cumulative task deficit closes],
    [NRR],
      [$1 - "MTTR" \/ "MTBF"$ — normalized recovery ratio @or2025mttra],
    [Critical Time],
      [Fraction of post-fault ticks below 50% baseline throughput],
    [Survival Rate],
      [Alive agents / initial fleet at end],
    [Propagation Rate],
      [Avg.~fraction of fleet affected per fault event],
    [Impacted Area],
      [Cumulative task deficit as % of baseline],
  ),
  caption: [Eight differential resilience metrics (12 total with
  sub-metrics). All require a paired baseline.],
) <tab-metrics>

An Agent Dependency Graph (ADG) — a directed graph where agent $i$
depends on agent $j$ if $j$ blocks $i$'s planned path — estimates
cascade depth via BFS after each tick @hoenig2019persistent.


// ─────────────────────────────────────────────────────────────
= Solvers

Five lifelong solvers are implemented from the literature in Rust (no
external solver crates):

#figure(
  table(
    columns: (auto, 1fr),
    stroke: 0.4pt,
    align: (left, left),
    inset: 5pt,
    table.header([*Solver*], [*Reference & Characteristic*]),
    [PIBT],
      [@okumura2022priority — One-step reactive, $O(n log n)$],
    [RHCR (3 variants)],
      [@li2021rhcr — Windowed PBS, PIBT-Window, or Priority A\*],
    [Token Passing],
      [@ma2017lifelong — Sequential planning via shared token],
  ),
  caption: [Lifelong MAPF solvers. One-shot solvers (CBS, LaCAM, LNS2)
  are excluded; fault resilience requires sustained operation.],
) <tab-solvers>


// ─────────────────────────────────────────────────────────────
= Demonstration: Fault-Induced Congestion Relief

To demonstrate MAFIS's measurement capabilities, we report a
large-scale experiment revealing solver- and topology-dependent behavior
undetectable without differential measurement infrastructure.

== Experimental Setup

_Configuration._ 5 solvers × 4 fleet densities (10, 20, 40, 80) × 7
fault scenarios × 50 seeds = 7,000 paired runs on `warehouse_medium`
(32 × 21). Cross-topology validation: 480 additional runs on
`sorting_center` and `compact_grid`. Scheduler: random (delivery
targets drawn uniformly to eliminate positional bias). Tick count: 500.
All runs headless with Rayon parallelism (~90s total).

_Statistical method._ Per paired run: Braess ratio = $overline("throughput")_"fault" \/ overline("throughput")_"baseline"$. Two-sided
Mann-Whitney $U$ with Benjamini-Hochberg FDR correction across all 140
tests @benjamini1995controlling. Cliff's delta for effect sizes. A
finding is _confirmed_ when 95% CI lower bound > 1.0 AND BH-adjusted
$p < 0.05$.

== Results

Of 140 hypothesis tests, 35 are significant after BH correction; 9
show confirmed congestion relief (ratio > 1, CI > 1, adjusted $p <
0.05$). All 9 occur under burst failure scenarios.

#figure(
  table(
    columns: (auto, auto, auto, auto, auto),
    stroke: 0.4pt,
    align: (left, center, center, center, center),
    inset: 5pt,
    table.header(
      [*Solver*], [*Threshold*], [*Ratio*], [*Cliff's $delta$*], [*$p_"adj"$*],
    ),
    [PIBT],          [$n=10$],  [1.774 \[1.33, 2.22\]], [0.376], [0.006],
    [RHCR-PIBT],     [$n=80$],  [1.608 \[1.33, 1.89\]], [0.347], [0.015],
    [RHCR-PBS],      [none],    [—], [—], [—],
    [RHCR-A\*],      [none],    [—], [—], [—],
    [Token Passing], [none],    [0.688 \[0.64, 0.73\]], [$-$0.991], [\<0.001],
  ),
  caption: [Braess ratios under burst-20% at the confirmed threshold
  density. _Threshold_ = lowest $n$ where BH-adjusted $p < 0.05$ and
  CI lower > 1. Token Passing shows no congestion relief at any density.
  RHCR-PBS and RHCR-A\* are never confirmed.],
) <tab-braess>

The phenomenon parallels Braess's paradox @braess1968paradoxon: removing
capacity (killing agents) improves flow for solvers operating in a
congested regime. PIBT is locally reactive — each agent optimizes
greedily, analogous to selfish routing. The effect appears from $n=10$
and peaks at $n=80$ (2.09×). RHCR-PIBT resists until $n=80$, requiring
extreme density before coordination breaks down. Token Passing's
sequential token structure is fragile: each death creates a path gap
that propagates through the shared constraint set, producing Cliff's
$delta = -0.99$ to $-1.00$ at low density.

Jiang et al. @jiang2024scaling observed throughput saturation in lifelong
MAPF and used intentional agent disabling as an optimization trick. Our
result extends this: _unintentional_ faults produce the same effect, and
the response is architecture-dependent.

== Cross-Topology Validation

The effect is _topology-dependent_. On `compact_grid` (Ocado-style),
PIBT at $n=40$ confirms: ratio 1.431 \[1.33, 1.54\], $p < 0.001$. On
`sorting_center` (3 chokepoints), no density produces a confirmed
effect. This indicates the mechanism requires corridor congestion (many
alternative routes competing), not structural bottlenecks (few chokepoints
limiting throughput). Token Passing collapses on both topologies (8/8
configs below 1.0, all $p < 0.001$).


// ─────────────────────────────────────────────────────────────
= Observatory Features

_3D Visualization._ Instanced meshes render up to 500 agents in one draw
call. Color palettes encode task state (simple: 4 states; detailed: 8).
Dual heatmaps (density decay + cumulative traffic) identify hotspots.

_Timeline & Rewind._ Ring-buffer tick history; scrub to any past tick.
Fault events appear as annotated markers.

_Experiment Infrastructure._ Headless runner produces CSV, JSON, LaTeX,
Typst, and SVG outputs. Python analysis script computes BH-corrected
significance without external dependencies.

_Topologies._ Five industry-inspired layouts (Amazon Kiva small/medium/
large, FedEx sorting center, Ocado compact grid) defined as JSON.


// ─────────────────────────────────────────────────────────────
= Related Work

_Solver evaluation tools._ MAPF Tracker evaluates one-shot solver
speed on clean benchmarks. The League of Robot Runners @jiang2024scaling
evaluates lifelong throughput under normal conditions. SMART
provides kinodynamic fidelity for execution. None inject faults or
measure resilience. WareRover couples scheduling and MAPF with a basic
fault model (flat probability, binary state, one-shot solvers); MAFIS
provides Weibull-calibrated wear, three fault categories, five lifelong
solvers, cascade analysis, and browser deployment.

_Execution robustness._ Hönig et al.~@hoenig2019persistent introduce ADG
for delay-robust execution; Atzmon et al.~@atzmon2020robust define
$k$-robust MAPF. Morag et al.~@morag2023adapting handle planning
failures in lifelong MAPF. All address delays or planner timeouts —
temporary disruptions. MAFIS addresses permanent failures: robot death,
zone loss, progressive Weibull wear.

_Congestion in MAPF._ Chen et al.~@chen2024traffic optimize traffic flow
for lifelong MAPF, documenting throughput saturation. Atasoy Bingol et
al.~@atasoy2025retrograde formalize "retrograde scalability" in swarm
robotics. MAFIS extends these observations to the fault domain: we show
that faults can push a congested system past the throughput peak.

_Resilience in MAS._ Vistbakka & Troubitsyna @vistbakka2021modelling
formalize resilient reconfiguration; Zahradka et al.~@zahradka2025holistic
monitor MAPF execution via ADG with 1,300 experiments on one-shot MAPF.
MAFIS operates on lifelong MAPF with 7,480 runs and browser access.


// ─────────────────────────────────────────────────────────────
= Conclusion

MAFIS provides the first browser-based fault resilience observatory for
lifelong MAPF. Its three-category taxonomy, twelve differential metrics,
and deterministic replay enable studies impossible with existing tools.
The demonstration experiment — revealing topology-dependent congestion
relief under fault injection, confirmed with BH-FDR correction — shows
that MAFIS can surface non-obvious behaviors warranting targeted solver
design.

_Limitations._ Solvers are reimplemented (not reference code). Maximum
tested density is 80 agents; scale validation is future work. The
congestion relief effect requires formal modeling to explain the
mechanism — planned for a follow-up study.

_Live demo:_
#link("https://stasis-website.vercel.app/simulator")[stasis-website.vercel.app/simulator] ·
#link("https://github.com/stasis-industries/mafis")[github.com/stasis-industries/mafis]

// ─────────────────────────────────────────────────────────────
#bibliography("refs.bib", title: "References", style: "association-for-computing-machinery")

] // end columns
