# Audit Report: MAFIS PAAMS 2026 Paper

**Paper:** "MAFIS: A Resilience Observatory for Lifelong Multi-Agent Path Finding"  
**Venue:** PAAMS 2026 — AREA Workshop (Springer LNCS)  
**Author:** Teddy Truong, Université du Québec à Chicoutimi  
**Audit Date:** 2026-05-16  

---

## Executive Summary

The paper is technically sound overall. All 20+ references exist and support their attributed claims. The six differential metrics match their code implementations exactly. The 4,320 paired-run count is confirmed by both code assertions and data files. The novelty gap ("no prior tool injects controlled faults and reports differential resilience metrics for lifelong MAPF") is valid. Solver provenance is well-documented with traceable audit trails.

**One MAJOR issue found:** Table 5 (RQ2) contains factual errors — it lists scenarios as paradoxical (FT > 1.2) that the data does not support, and understates the range of observed values. **Two MINOR issues** found regarding completeness of related work and a Rapidity metric implementation detail not mentioned in the paper text.

---

## Findings by Task

### T1: Reference Validity — PASS ✅

All 20+ cited references verified via web search. Key verifications:

| Reference | Status | Source |
|-----------|--------|--------|
| li2021rhcr | ✅ | Confirmed as arXiv cs.AI paper by Jiaoyang Li et al. |
| okumura2022pibt | ✅ | AI Journal 310:103752, 2022 |
| ma2017tp | ✅ | arXiv 1705.10868, Lifelong MAPD with Token Passing |
| chan2024lorr | ✅ | ICAPS 2024 Demo Track [[1]](https://openreview.net/forum?id=mPmCnEHTvJ) |
| lorr2026website | ✅ | Active competition at [leagueofrobotrunners.org](https://www.leagueofrobotrunners.org/), CFP confirms stochastic delays [[2]](https://groups.google.com/g/rl-list/c/gaXzQX5xNqg) |
| yan2026lsmart | ✅ | arXiv 2602.15721, GitHub repo at [smart-mapf/lifelong-smart](https://github.com/smart-mapf/lifelong-smart) [[3]](https://arxiv.org/abs/2602.15721) |
| atzmon2018robust | ✅ | SoCS 2018 pp. 2-9, AAAI proceedings [[4]](https://ojs.aaai.org/index.php/SOCS/article/view/18445) |
| atzmon2020probrobust | ✅ | ICAPS 2020 pp. 29-37 [[5]](https://ojs.aaai.org/index.php/ICAPS/article/view/6642) |
| hoenig2019robust | ✅ | IEEE RA-L 4(2):1125-1131, 2019 [[6]](https://whoenig.github.io/publications/2019_RA-L_Hoenig.pdf) |
| zhang2024metmapf | ✅ | ACM TOSEM 33(8) Article 198, DOI 10.1145/3669663 [[7]](https://openreview.net/forum?id=AYOQckZDm1) |
| bruneau2003resilience | ✅ | Earthquake Spectra 19(4):733-752, 2003 [[8]](http://ui.adsabs.harvard.edu/abs/2003EarSp..19..733B/abstract) |
| woods2015brittleness | ✅ | RESS 141:5-9, 2015 [[9]](https://www.sciencedirect.com/science/article/abs/pii/S0951832015000848) |
| basiri2016chaos | ✅ | IEEE Software 33(3):35-41, 2016, DOI 10.1109/MS.2016.60 [[10]](https://researchr.org/publication/BasiriBRHKRR16) |
| motter2002cascade | ✅ | Phys Rev E 66:065102(R), 2002 [[11]](https://journals.aps.org/pre/abstract/10.1103/PhysRevE.66.065102) |
| kannan2006metrics | ✅ | PerMIS 2006, NIST SP 1062 [[12]](https://web.eecs.utk.edu/~leparker/publications/PERMIS_06.pdf) |
| carlson2005ugv | ✅ | IEEE Trans Robotics 21(3):423-437, 2005 [[13]](http://www.cs.cmu.edu/~reliability/) |
| avizienis2004taxonomy | ✅ | IEEE TDSC 1(1):11-33, 2004 |
| meyer1980performability | ✅ | IEEE TC C-29(8):720-731, 1980 |
| hollnagel2006resilience | ✅ | Ashgate book, ISBN 9780754646419 |
| stern2019mapf | ✅ | SoCS 2019 pp. 151-158 |

**Claim accuracy:** Each reference is used correctly. Bruneau 2003 for resilience triangle, Woods 2015 for graceful extensibility, Basiri 2016 for chaos engineering protocol, Hönig 2019 for ADG and runtime replanning — all accurately attributed.

### T2: Metric Formula Alignment — PASS ✅

Cross-checked each formula in the LaTeX against `src/experiment/metrics.rs`:

| Metric | Paper Formula | Code Implementation | Verified |
|--------|--------------|---------------------|----------|
| **FT** | P̄_f / P̄_b over [t_f, T] | `avg(throughput_series[start..])` for faulted/baseline (line 215-240) | ✅ Exact match |
| **CT** | \|{t: P_f(t) < 0.5·P_b(t)}\| / (T-t_f) | Uses `CRITICAL_TIME_THRESHOLD=0.5` from constants.rs:277 (line 410-437) | ✅ Exact match |
| **ITAE** | Σ(t-t_f)·\|1 - R(t)\| | Line 443-470, skips zero-baseline ticks | ✅ Exact match |
| **AR** | \|ever-affected\| / \|initial fleet\| | Computed via `ever_affected: Vec<bool>` in fault pipeline | ✅ Exact match |
| **Cascade Depth** | mean(max BFS depth per event) | `IndexedDependencyGraph` + `cascade_bfs_standalone` in cascade.rs | ✅ Exact match |
| **Rapidity** | T_rec(ρ=0.9), 5 consec. ticks | `RAPIDITY_THRESHOLD=0.9`, `RAPIDITY_DWELL=5` (constants.rs:290-294) | ✅ Exact match |

**Implementation note:** The Rapidity code (line 485-525) includes a "degradation-observed gate" — returns NaN if the throughput ratio is already ≥ 0.9 at fault onset. This prevents spurious Rapidity=0 when no degradation is actually observed. Sound engineering, but not documented in the paper's metric definition.

### T3: Experimental Run Count — PASS ✅

Paper claim: 4,320 paired runs.

Verification via `src/experiment/suite.rs`:
- E1a (warehouse_single_dock): 3 solvers × 6 scenarios × 3 counts (20/40/60) × 30 seeds = **1,620**
- E1b (warehouse_dual_dock): 3 solvers × 6 scenarios × 3 counts (40/80/120) × 30 seeds = **1,620**
- E2 (scheduler_effect): 3 solvers × 6 scenarios × 2 schedulers × 1 count (40) × 30 seeds = **1,080**
- **Total: 4,320** ✅

Code assertion at suite.rs:614: `assert_eq!(total, 4320);` — passes in test suite.

### T4: Solver Provenance — PASS ✅

Each solver file includes a header with dated audit trail:

- **PIBT** (`src/solver/pibt/solver.rs`): Cites Okumura et al. AI 310:103752 (2022). Uses shared `PibtCore` module.
- **RHCR-PBS** (`src/solver/rhcr/solver.rs`): "Audited 2026-04-06" with line-level mappings to [Jiaoyang-Li/RHCR](https://github.com/Jiaoyang-Li/RHCR.git):
  - `update_congestion()` → `BasicSystem::congested()` at BasicSystem.cpp:310
  - `fill_goal_sequences()` → `KivaSystem::update_goal_locations()` at KivaSystem.cpp:70
- **Token Passing** (`src/solver/token/token_passing.rs`): "Audited 2026-04-07" with structural mapping table (pibt2 absolute-time ↔ MAFIS relative-time data structures). Reference: [Kei18/pibt2](https://github.com/Kei18/pibt2.git).

All cited GitHub repos are public and contain the referenced files.

### T5: Novelty/Gap Claim — PASS ✅

Paper's gap: "None inject controlled faults or report differential resilience metrics [for lifelong MAPF]."

Systematic search found no contradicting tool:

1. **Yoeli, Stern, Atzmon (SoCS 2019)** [[14]](https://ojs.aaai.org/index.php/SOCS/article/view/18494): "Measuring the Vulnerability of a MAPF Solution" — 2-page extended abstract. Measures vulnerability of a one-shot solution to agent removal. Not lifelong, no fault injection, no differential baseline-paired metrics.
2. **MAS-FIRE (arXiv 2602.19843)**: Fault injection for LLM-based multi-agent systems. Different domain entirely.
3. **Fault-tolerant MAPP (arXiv 2211.13908)**: Theoretical crashable-agent planning. Design-time robustness, not evaluation framework.
4. **WareRover (arXiv 2602.13999)**: Warehouse MAPF platform coupling OS+MAPF. Tests coupled constraints, not controlled fault injection with differential metrics.

**Assessment:** Gap claim is valid. No prior tool combines controlled fault injection + lifelong MAPF + paired-baseline differential metrics.

### T6: RQ2 (FT > 1.2) Verification — MAJOR ISSUE ⚠️

**Qualitative claim confirmed:** FT > 1.2 occurs exclusively under RHCR-PBS at saturating density. Zero such cells for PIBT or Token Passing across all configurations.

**Table 5 contains factual errors** (verified against `results/aisle_width/` summary CSVs):

| Paper Table 5 Entry | Actual Data | Status |
|---------------------|-------------|--------|
| SD-w1 n=60: "Wear-Med" listed as FT > 1.2 | Wear-Med FT = **0.95** | ❌ **FALSE** |
| SD-w1 n=60: FT range "1.21–1.66" | Burst-20=1.98, Zone=1.48, Intermittent=1.33 | ❌ **Range should be 1.33–1.98** |
| SD-w2 n=108: "Zone-Outage" listed as FT > 1.2 | Zone-Outage FT = **0.96** | ❌ **FALSE** |
| SD-w2 n=108: FT range "1.24–1.31" | Only Intermittent=1.22 qualifies | ❌ **Range should be just ~1.22** |
| SD-w3 n=151: Only "Intermittent" listed (1.32) | Also Zone=1.84, Burst-20=1.66, Burst-50=1.90 qualify | ⚠️ **Understated** |

Data source: `results/aisle_width/aisle_width_w1_summary.csv` (column 14 = ft_mean), confirmed in both pre-fix and post-fix versions of the data.

**Impact:** The qualitative finding and solver-specificity claim remain valid. Table 5 needs regeneration from final data.

### T7: Statistical Methodology — PASS ✅

- **30 seeds:** Standard for simulation studies. Paper acknowledges underpowered at ceiling/floor.
- **Student-t CI:** Correctly implemented in `src/experiment/stats.rs` with lookup table (df 1–30) + z-approx.
- **Cliff's delta:** Romano et al. 2006 thresholds (|d| < 0.147 negligible, < 0.33 small, < 0.474 medium).
- **Baseline validity filter:** Excludes cells where baseline < 30% best solver. Sound.
- **No Bonferroni:** Acceptable for exploratory analysis with CI reporting.

### T8: Limitation Honesty — PASS ✅

All stated limitations verified as accurate:
- Grid-world with unit-cost synchronised ticks ✅
- Fault parameters not calibrated against warehouse-AGV data ✅ (explicitly stated)
- Largest fleet n=120 modest vs production ✅
- Cross-machine ULP difference ✅
- Token Passing ≤100 cap documented ✅
- 30 seeds underpowered at extremes ✅

---

## Issue Summary

### MAJOR

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 1 | **Table 5 factual errors**: Wear-Med (SD-w1 n=60) not > 1.2; Zone-Outage (SD-w2 n=108) not > 1.2; FT ranges incorrect | §5.2, Table 5 | Regenerate table from final data |

### MINOR

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 2 | **Missing related work**: Yoeli et al. (SoCS 2019) "Measuring the Vulnerability of a MAPF Solution" not cited | §2.2 | Add one sentence noting it's tangential |
| 3 | **Rapidity gate undocumented**: Code returns NaN when no degradation observed at fault onset | §4.3 | Add parenthetical to metric definition |

### NEGLIGIBLE

| # | Issue | Location | Note |
|---|-------|----------|------|
| 4 | "Six of 150 cells" count needs recount | §5.2 text | Recount after fixing Table 5 |
| 5 | LoRR 2026 competition still running | Table 1 | Low risk — CFP is public record |

---

## Overall Verdict

| Aspect | Verdict |
|--------|---------|
| References exist and are correctly attributed | **PASS** |
| Metric formulas match code implementation | **PASS** |
| Experimental run count verified | **PASS** |
| RQ1 table values match data | **PASS** |
| RQ2 qualitative claim (RHCR-PBS only) | **PASS** |
| RQ2 Table 5 specific values | **FAIL** |
| Novelty gap claim | **PASS** |
| Solver provenance | **PASS** |
| Statistical methodology | **PASS** |
| Limitations accurately stated | **PASS** |

---

## Recommended Actions (Priority Order)

1. **CRITICAL (before submission):** Regenerate Table 5 from `results/aisle_width/` summary data. Correct criteria:
   - SD-w1 n=60: Remove "Wear-Med", keep Zone-Outage/Intermittent/Burst-20. Range: 1.33–1.98.
   - SD-w2 n=108: Remove "Zone-Outage", keep only Intermittent (FT≈1.22).
   - SD-w3 n=151: Add Zone-Outage (1.84), Burst-20 (1.66), and potentially Burst-50 (1.90) if not filtered by baseline validity.
   - Recount total paradoxical cells and update "six of 150" in text.

2. **SUGGESTED:** Add to §2.2: "Yoeli et al.~[SoCS 2019] measure one-shot solution vulnerability to agent removal but do not inject runtime faults or report differential baseline-paired metrics."

3. **SUGGESTED:** Add to §4.3 Rapidity definition: "(undefined when no degradation is observed at fault onset)."

---

## Sources

1. LoRR ICAPS 2024: https://openreview.net/forum?id=mPmCnEHTvJ
2. LoRR 2026 CFP: https://groups.google.com/g/rl-list/c/gaXzQX5xNqg
3. LSMART arXiv: https://arxiv.org/abs/2602.15721
4. Atzmon 2018: https://ojs.aaai.org/index.php/SOCS/article/view/18445
5. Atzmon 2020: https://ojs.aaai.org/index.php/ICAPS/article/view/6642
6. Hönig 2019: https://whoenig.github.io/publications/2019_RA-L_Hoenig.pdf
7. MET-MAPF: https://openreview.net/forum?id=AYOQckZDm1
8. Bruneau 2003: http://ui.adsabs.harvard.edu/abs/2003EarSp..19..733B/abstract
9. Woods 2015: https://www.sciencedirect.com/science/article/abs/pii/S0951832015000848
10. Basiri 2016: https://researchr.org/publication/BasiriBRHKRR16
11. Motter 2002: https://journals.aps.org/pre/abstract/10.1103/PhysRevE.66.065102
12. Kannan 2006: https://web.eecs.utk.edu/~leparker/publications/PERMIS_06.pdf
13. Carlson 2005: http://www.cs.cmu.edu/~reliability/
14. Yoeli 2019 (not cited in paper): https://ojs.aaai.org/index.php/SOCS/article/view/18494
