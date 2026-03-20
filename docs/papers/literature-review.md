# Literature Review — Resilience in Multi-Agent Systems

**Date:** 2026-03-11
**Purpose:** Position MAFIS against the state of the art, identify citable papers, and derive implementation priorities.

---

## MAFIS Positioning Statement

> *There exists no runtime tool that measures how lifelong multi-agent pathfinding systems degrade, recover, and adapt under configurable fault conditions in real-time.*

MAFIS is a **fault resilience observatory** — not a solver benchmark (that's MAPF Tracker's domain), not a formal verification tool, not a control-theoretic proof system. It is the **empirical instrument** that bridges the gap between theoretical resilience frameworks and observable, measurable fault behavior in spatial multi-agent coordination.

The surrounding literature validates the problem space (resilience in MAS is an active research area) but none of the existing work builds the observation instrument MAFIS provides.

---

## Paper Summaries & Citations

### Paper 1 — Resilient Consensus Control for Linear MAS Against False Data Injection Attacks

| Field | Value |
|-------|-------|
| **Authors** | Meirong Wang, Jianqiang Hu, Jinde Cao |
| **Published** | Int. J. Control, Automation, and Systems 21(7), 2023 |
| **DOI** | 10.1007/s12555-022-0261-y |
| **File** | `docs/papers/Resilient Consensus Control.pdf` |

**Summary:** Studies resilient consensus in linear MAS under False Data Injection Attacks (FDIAs). Designs an extended state observer per agent that simultaneously estimates true state and injected false data, then uses it to build a distributed resilient controller. Proves sufficient conditions for consensus under bounded/decaying attacks (undirected topology) and necessary+sufficient conditions for directed topology. Validated on 10-agent networks.

**Key concepts:**
- Extended state observer to detect AND estimate injected false data
- Two attack models: bounded linear (Assumption 1: `d'(t) = Wd(t)`) and general decaying (Assumption 2)
- Consensus recovery via Lyapunov stability + LMI conditions
- Undirected vs directed communication topology affects resilience conditions

**Cite for:**
- Establishing that fault injection into inter-agent communication is a studied MAS resilience problem
- The observer-based detection pattern (analogous to our baseline-differential detection)
- Theoretical grounding that communication topology affects fault resilience
- Context: "Prior work focuses on consensus control under attacks in continuous-state linear MAS [Wang2023]; we address the spatial, discrete-time, lifelong MAPF setting where faults affect physical movement rather than state estimates."

**Gap MAFIS fills:** This paper operates in continuous-state abstract space with no physical/spatial agents, no visualization, and no empirical warehouse scenarios. MAFIS provides the observable, spatial counterpart.

---

### Paper 2 — Modelling Resilient Collaborative Multi-Agent Systems

| Field | Value |
|-------|-------|
| **Authors** | Inna Vistbakka, Elena Troubitsyna |
| **Published** | Computing (Springer) 103, pp. 535-557, 2021 |
| **DOI** | 10.1007/s00607-020-00861-2 |
| **File** | `docs/papers/Modelling Resilient Collaborative Systems.pdf` |

**Summary:** Proposes a formal framework for resilience-explicit modelling of collaborative MAS using Event-B (state-based formal methods with refinement). Defines agents with capabilities, goals, dynamic relationships, and active/inactive health status. Reconfigurability = ability to redistribute tasks from failed to healthy agents. Case study: smart warehouse with robots, charging stations, collision avoidance, and task reassignment.

**Key concepts:**
- **Agent Capabilities (AC)**: relation between agents and their functional capabilities
- **Goal-Capability mapping (GC_Rel)**: which capabilities are needed for which goals
- **Active/Inactive status**: tracks agent health; inactive agents can't participate in collaborative activities
- **Three resilience levels:**
  - Local: individual agent handles own transient faults
  - Structural: new collaborations formed between agents
  - Compensating: new agents or capabilities introduced
- **Reconfiguration properties:**
  - Property 1: Active agents do collaborative activities; inactive do individual only
  - Property 2: Only relationship-linked agents participate in collaborative activities
  - Property 3: Agents in a collaboration must have required capabilities
- **LoseCapability / RestoreCapability** events model failure and recovery
- **Smart warehouse case study**: robots, charging stations, collision avoidance, task reassignment on failure

**Cite for:**
- The formal definition of resilience as "progressing toward goals despite changes in internal state and environment"
- The capability/reconfigurability framework (agents lose capability → tasks redistributed)
- Local vs system-level resilience distinction (maps to our per-agent vs system-level metrics)
- The warehouse case study that mirrors MAFIS's domain
- Context: "Vistbakka & Troubitsyna [2021] formalize resilient reconfiguration in MAS using Event-B, proving correctness of task redistribution after failures; MAFIS instantiates this framework empirically, measuring the quantitative cost of reconfiguration (throughput loss, recovery time, cascade propagation) in real-time lifelong MAPF simulations."

**Gap MAFIS fills:** Event-B is qualitative — it proves that reconfiguration is possible but does not measure *how much it costs* (throughput loss, recovery time, cascade depth). MAFIS provides quantitative measurement.

---

### Paper 3 — MTTR-A: Measuring Cognitive Recovery Latency in Multi-Agent Systems

| Field | Value |
|-------|-------|
| **Authors** | Barak Or |
| **Published** | arXiv:2511.20663v5, Dec 2025 |
| **File** | `docs/papers/MTTR-A Measuring Cognitive Recovery Latency in Multi-Agent Systems.pdf` |

**Summary:** Introduces MTTR-A (Mean Time-to-Recovery for Agentic Systems), a runtime reliability metric for cognitive recovery latency in LLM-based MAS. Adapts classical dependability theory (MTTR, MTBF, availability) to agentic orchestration. Defines a taxonomy of 5 reflex families for recovery. Proves two theorems linking NRR (Normalized Recovery Ratio) to steady-state cognitive uptime. Empirically validated with 200 LangGraph runs on AG News dataset.

**Key metrics defined:**
- **MTTR-A_sys**: Mean recovery time across agents = `(1/N) Σ MTTR-A_i`
- **MedTTR-A**: Median recovery time (robust to outliers)
- **MTBF_sys**: Mean time between cognitive faults
- **NRR_sys**: `1 - MTTR-A_sys / MTBF_sys` (normalized recovery ratio, 0-1)
- **Latency decomposition**: `ΔT = T_detect + T_decide + T_execute`

**Reflex taxonomy (5 families):**
1. Recovery: auto-replan, rollback, tool-retry, fallback-policy, safe-mode
2. Human-in-the-loop: approve, override, review, escalate
3. Runtime control: auto-diagnose, self-heal, confidence-gate, vote/consensus, sandbox-execute
4. Coordination: broadcast-update, negotiate-task, sync-state, lock/release
5. Safety: graceful-abort, force-terminate, audit-snapshot

**Theoretical results:**
- **Theorem 1**: NRR_sys ≥ 1 - λ·μ is a conservative lower bound on steady-state cognitive uptime π_up (via alternating-renewal model)
- **Theorem 2**: Variance-aware bound NRR_α incorporating recovery-time uncertainty (via Cantelli's inequality)

**Empirical results** (200 LangGraph runs, AG News):
- MedTTR-A = 6.21s ± 2.14s, MTBF = 6.73s, NRR = 0.077
- tool-retry fastest (4.46s), human-approve slowest (12.22s)

**Cite for:**
- The MTTR-A / MTBF / NRR metric family — directly adaptable to tick-based measurement in MAFIS
- The latency decomposition (detect + decide + execute) — maps to our fault detection → replanning → path execution pipeline
- The reflex taxonomy — vocabulary for classifying MAFIS fault-recovery mechanisms
- Theorem 1 (NRR as uptime lower bound) — could be applied to MAFIS's recovery metrics
- Context: "Or [2025] defines MTTR-A for cognitive recovery in LLM-based MAS; we adapt this metric family to spatial multi-agent pathfinding, computing MTTR in discrete ticks and linking it to throughput recovery rather than reasoning coherence."

**Gap MAFIS fills:** MTTR-A targets LLM reasoning drift, not physical/spatial MAS. No grid world, no MAPF, no throughput measurement. MAFIS computes analogous metrics in a spatial coordination context and provides the visual instrument to observe recovery in real time.

---

### Paper 4 — TRUST-LAPSE: An Explainable and Actionable Mistrust Scoring Framework for Model Monitoring

| Field | Value |
|-------|-------|
| **Authors** | Nandita Bhaskhar, Daniel L. Rubin, Christopher Lee-Messer |
| **Published** | IEEE Trans. on Artificial Intelligence 5(4), pp. 1473-1485, 2023 |
| **File** | `docs/papers/An Explainable and Actionable Mistrust Scoring Framework for Model Monitoring.pdf` |

**Summary:** Proposes TRUST-LAPSE, a "mistrust" scoring framework for continuous model monitoring. Uses latent-space embeddings to score each prediction's trustworthiness. Two-layer approach: (a) latent-space mistrust score using Mahalanobis distance + cosine similarity to a coreset, and (b) sequential mistrust score using a sliding-window Mann-Whitney test to detect drift over time. Achieves SOTA on audio, EEG, and vision benchmarks.

**Key concepts:**
- **Latent-space mistrust score (s_LSS)**: Product of distance score (Mahalanobis) × similarity score (cosine) in latent space
- **Sequential mistrust score (s_mis)**: Sliding-window comparison (reference window from coreset vs sliding window from recent predictions) using Mann-Whitney test
- **Three desiderata**: post-hoc, explainable, actionable
- **Semantic sensitivity**: unlike baselines, TRUST-LAPSE is sensitive to semantic content, not just dataset statistics
- **Drift detection**: >90% of streams show <20% error across all domains

**Cite for:**
- The sliding-window-vs-baseline drift detection concept — directly parallels MAFIS's baseline-differential measurement
- The "trust scoring" methodology — agents whose behavior drifts from baseline could be scored similarly
- The explainability angle — MAFIS's heatmaps and cascade visualization serve the same purpose (making abstract metrics tangible)
- Context: "Bhaskhar et al. [2023] detect performance drift in deployed ML models via sequential comparison to a reference baseline; we apply an analogous baseline-differential principle to multi-agent coordination, comparing live simulation metrics against a deterministic fault-free baseline at every tick."

**Gap MAFIS fills:** TRUST-LAPSE monitors single-model predictions, not multi-agent spatial coordination. The baseline-comparison principle is shared, but the domain (ML model monitoring vs MAPF resilience) is entirely different.

---

## Cross-Paper Synthesis

### Concepts that validate MAFIS's approach

| Concept | Source Papers | MAFIS Implementation |
|---------|-------------|------------------------|
| Recovery latency as first-class metric | Paper 3 (MTTR-A) | MTTR in ticks, recovery detection, cumulative gap |
| Baseline-differential measurement | Paper 4 (TRUST-LAPSE) | Headless baseline vs live at every tick |
| Fault detection via state estimation | Paper 1 (Observer) | Baseline divergence = fault impact detection |
| Formal resilience model (capabilities, reconfiguration) | Paper 2 (Event-B) | Task redistribution after agent death |
| Resilience ≠ robustness | Papers 2, 3 | Scorecard separates robustness (absorption) from recoverability (adaptation) |
| Recovery reflex taxonomy | Paper 3 (5 families) | Auto-replan (solver), rollback (rewind), safe-mode (degraded operation) |
| Sliding window for drift detection | Paper 4 (Mann-Whitney) | Could add statistical drift detection to metrics |
| Topological resilience dependence | Paper 1 (directed vs undirected) | Warehouse vs open floor produce different resilience profiles |

### What MAFIS uniquely provides (not in any paper)

1. **Real-time 3D observation** of fault propagation in spatial MAS
2. **Lifelong MAPF + fault injection** — the bridge between MAPF benchmarking and resilience research
3. **Configurable fault scenarios** as a first-class research variable
4. **Cascade visualization** (ADG + BFS + heatmaps) — structural fragility made visible
5. **Braess's Paradox detection** — positive deviation as diagnostic signal
6. **Multiple solver/scheduler combinations** as experimental variables
7. **Deterministic rewind** for replay and counterfactual analysis

---

## Implementation Priorities (State-of-the-Art Alignment)

### Priority 1 — Adopt MTTR-A metric family (from Paper 3)

**What:** Compute MTTR-A, MTBF, and NRR in MAFIS using tick-based measurement.

**Why:** Paper 3 provides formal backing for our recovery metrics. Adopting the same metric names and formulas strengthens citability and positions MAFIS as the spatial-MAPF instantiation of Or's framework.

**Implementation:**
- [ ] Compute per-agent MTTR-A_i: ticks from fault detection to path resumption
- [ ] Compute system-level MTTR-A_sys: mean across all agents
- [ ] Compute MTBF_sys: mean ticks between fault events
- [ ] Compute NRR_sys: `1 - MTTR-A_sys / MTBF_sys`
- [ ] Decompose recovery into: T_detect (ticks until replanning starts) + T_execute (ticks until agent resumes moving toward goal)
- [ ] Add to bridge JSON and Results Dashboard
- [ ] Add to export CSV

**Cite:** Or [2025] — "We instantiate the MTTR-A framework [Or2025] in discrete-time spatial MAPF, replacing continuous-time cognitive recovery with tick-based path recovery measurement."

---

### Priority 2 — Implement baseline-differential metrics (from Paper 4's principle)

**What:** Complete the metrics redesign from `docs/metrics-redesign-brainstorm.md`.

**Why:** Paper 4 validates the baseline-comparison principle we've already designed. Implementing it fully positions MAFIS alongside TRUST-LAPSE's methodology, adapted for spatial MAS.

**Implementation:**
- [ ] `BaselineDiff` resource with gap, deficit_integral, surplus_integral, net_integral
- [ ] Cumulative catch-up recovery detection (parameter-free)
- [ ] Rate recovery detection (secondary)
- [ ] Remove self-referential throughput metrics
- [ ] Update scorecard: Recoverability = gap-growth ratio, Degradation Slope = gap regression
- [ ] Rewind correctness: recompute BaselineDiff on rewind

**Cite:** Bhaskhar et al. [2023] — "Following the baseline-differential principle from trust scoring [Bhaskhar2023], we compare live simulation metrics against a deterministic fault-free baseline at every tick, treating deviation magnitude and recovery dynamics as the primary resilience indicators."

---

### Priority 3 — Formalize resilience vocabulary (from Paper 2)

**What:** Align MAFIS's documentation and metric naming with the formal resilience framework from Vistbakka & Troubitsyna.

**Why:** Paper 2 provides the formal vocabulary (capabilities, reconfigurability, active/inactive, local vs system-level resilience) that makes MAFIS's empirical results academically rigorous.

**Implementation:**
- [ ] Map MAFIS concepts to Paper 2's formal definitions in DEFINITIONS.md:
  - Agent capabilities → { move, carry, plan }
  - LoseCapability → agent death (permanent) or latency fault (temporary)
  - RestoreCapability → latency fault expiration
  - Reconfiguration → task reassignment + replanning by remaining agents
  - Local resilience → individual agent replanning around obstacle
  - System resilience → fleet-wide throughput recovery
- [ ] Add "Resilience Framework" section to DEFINITIONS.md citing Paper 2
- [ ] Categorize existing fault types by Paper 2's resilience levels

**Cite:** Vistbakka & Troubitsyna [2021] — "We adopt the resilience-explicit formalization from [Vistbakka2021], where agent capabilities, reconfigurability, and goal reachability provide the formal basis; MAFIS instantiates this framework as a runtime simulation, measuring the quantitative cost of reconfiguration in lifelong MAPF."

---

### Priority 4 — Add statistical drift detection (inspired by Paper 4)

**What:** Add a sliding-window statistical test to detect when live metrics have significantly diverged from baseline.

**Why:** Currently, MAFIS computes raw deviation. Adding a statistical significance test (Mann-Whitney or similar) would make fault-impact detection more rigorous, aligning with TRUST-LAPSE's approach.

**Implementation:**
- [ ] Sliding window of recent live throughput values vs corresponding baseline values
- [ ] Mann-Whitney or Wilcoxon test for significant divergence
- [ ] Binary "drift detected" / "recovered" signal based on p-value threshold
- [ ] Display in UI as drift indicator (separate from raw gap)
- [ ] Optional: per-zone drift detection using heatmap data

**Cite:** Bhaskhar et al. [2023] — "We adapt the sequential drift detection from [Bhaskhar2023] to the multi-agent coordination domain, using a sliding-window statistical test to detect when agent throughput has significantly diverged from the deterministic baseline."

---

### Priority 5 — NRR-based cognitive uptime bound (from Paper 3)

**What:** Compute and display the theoretical uptime bound from Theorem 1.

**Why:** Directly applying Or's theorem to MAFIS data gives a formal reliability guarantee that can be compared across configurations.

**Implementation:**
- [ ] After sufficient fault events, compute NRR_sys
- [ ] Apply Theorem 1: π_up ≥ NRR_sys (lower bound on fraction of ticks system operates at baseline)
- [ ] Display in Results Dashboard: "Estimated minimum uptime: X%"
- [ ] Apply Theorem 2 with variance for confidence interval
- [ ] Add to export data

**Cite:** Or [2025] — "By Theorem 1 of [Or2025], the computed NRR provides a conservative lower bound on the steady-state fraction of ticks during which the system operates at baseline throughput."

---

### Priority 6 — Recovery reflex classification (from Paper 3)

**What:** Classify MAFIS's fault-response mechanisms using Paper 3's reflex taxonomy.

**Why:** Provides standardized vocabulary for describing what happens after a fault, useful for paper writing and comparison.

**Mapping:**
| Paper 3 Reflex | MAFIS Equivalent |
|---------------|---------------------|
| auto-replan | Solver replanning after agent death (automatic) |
| rollback | Timeline rewind (manual, researcher-initiated) |
| tool-retry | N/A (no tool calls) |
| fallback-policy | RHCR fallback to PIBT on timeout |
| safe-mode | Degraded operation (latency fault reduces agent to Wait) |
| human-approve | Manual fault injection via context menu (researcher decides) |
| broadcast-update | Task reassignment broadcasts new goals to replanning agents |
| graceful-abort | Simulation stop on collapse detection |
| audit-snapshot | Tick history + rewind + CSV export |

**Implementation:**
- [ ] Add reflex classification to fault event records
- [ ] Display in fault timeline: which recovery mechanism was triggered
- [ ] Export reflex type in CSV data

---

## Additional Papers to Seek

For a complete literature review, consider adding:

1. **MAPF Tracker / BenchMARL** — solver benchmarking (to contrast: "they benchmark solvers under clean conditions; we measure what happens when things go wrong")
2. **Lifelong MAPF papers** (Li et al. 2021 — already in `docs/papers/`) — the MAPF foundation
3. **Fault-tolerant consensus control of MAS: A survey** (Gao et al. 2022, Int. J. Systems Science) — broader survey cited by Paper 3
4. **Resilience for the scalability of dependability** (Laprie 2005) — foundational resilience definition
5. **Site Reliability Engineering** (Beyer et al. 2020) — the SRE book that defines MTTR/MTBF classically

---

## Citation Block (BibTeX)

```bibtex
@article{wang2023resilient,
  title={Resilient Consensus Control for Linear Multi-agent System Against the False Data Injection Attacks},
  author={Wang, Meirong and Hu, Jianqiang and Cao, Jinde},
  journal={International Journal of Control, Automation, and Systems},
  volume={21},
  number={7},
  pages={2112--2123},
  year={2023},
  doi={10.1007/s12555-022-0261-y}
}

@article{vistbakka2021modelling,
  title={Modelling resilient collaborative multi-agent systems},
  author={Vistbakka, Inna and Troubitsyna, Elena},
  journal={Computing},
  volume={103},
  pages={535--557},
  year={2021},
  doi={10.1007/s00607-020-00861-2}
}

@article{or2025mttra,
  title={{MTTR-A}: Measuring Cognitive Recovery Latency in Multi-Agent Systems},
  author={Or, Barak},
  journal={arXiv preprint arXiv:2511.20663v5},
  year={2025}
}

@article{bhaskhar2023trustlapse,
  title={An Explainable and Actionable Mistrust Scoring Framework for Model Monitoring},
  author={Bhaskhar, Nandita and Rubin, Daniel L. and Lee-Messer, Christopher},
  journal={IEEE Transactions on Artificial Intelligence},
  volume={5},
  number={4},
  pages={1473--1485},
  year={2023}
}
```
