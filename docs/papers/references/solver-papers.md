# Solver Paper References

Papers, reference implementations, and key algorithmic details for each solver implemented in MAFIS.

---

## PIBT — Priority Inheritance with Backtracking

**Paper:** Okumura, Machida, Defago, Tamura, "Priority Inheritance with Backtracking for Iterative Multi-Agent Path Finding", Artificial Intelligence Journal (AIJ), 2022.
**arXiv:** [2104.05491](https://arxiv.org/abs/2104.05491) (extended from IJCAI 2019)
**Reference impl:** [github.com/Kei18/pibt2](https://github.com/Kei18/pibt2) (C++)

### Key Guarantees
- **Theorem 1:** The highest-priority agent always reaches its goal eventually (in well-connected graphs).
- **Proposition 1:** No vertex or edge collisions.
- Priority inheritance: when agent A pushes into agent B's cell, B inherits A's priority and is recursively assigned.

---

## RT-LaCAM — Real-Time LaCAM

**Paper:** Liang, Veerapaneni, Harabor, Li, Likhachev, "Real-Time LaCAM for Real-Time MAPF", SoCS 2025.
**arXiv:** [2504.06091](https://arxiv.org/abs/2504.06091)
**DOI:** 10.1609/socs.v18i1.35993
**Reference impl:** [github.com/ekusiadadus/rt-lacam](https://github.com/ekusiadadus/rt-lacam) (Zig, ~3000 lines, 86 tests)
**Related:** [github.com/Kei18/lacam](https://github.com/Kei18/lacam) (original LaCAM, C++), [github.com/Kei18/lacam2](https://github.com/Kei18/lacam2) (LaCAM*), [github.com/Kei18/lacam3](https://github.com/Kei18/lacam3) (Engineering LaCAM*)

### Algorithm (Section 3)
1. **Lazy DFS over configurations:** each HighLevelNode has a LowLevelNode constraint tree. Instead of generating all 5^N successors, generates one via PIBT. When a config is revisited, adds a constraint forcing PIBT to produce a different successor.
2. **PIBT as configuration generator:** applies LLN constraints as pre-decided positions, PIBT fills the rest.
3. **Rerooting:** when agents move A→B, swap parent pointer so B becomes root. Enables continued exploration without losing history.
4. **Persistent explored map:** grows monotonically across all step() calls.
5. **Path extraction:** backtrack from goal_node through parent chain to current_node. BFS fallback if parent chain is broken after reroot.

### Key Guarantees
- **Completeness:** RT-LaCAM builds the same search tree as full LaCAM (across iterations instead of all at once). Complete because full LaCAM is complete.
- **Runtime equivalence:** total planning time across all iterations equals full LaCAM's planning time (minus negligible reroot/backtrack overhead).

### Data Structures
- `HighLevelNode`: config (positions), parent, neighbors, LLN tree, priority order, g, h
- `LowLevelNode`: who (constrained agents), where (target positions), depth
- `explored: HashMap<ConfigHash, NodeId>` — persistent across ticks
- `open: VecDeque<NodeId>` — DFS open list

---

## TPTS — Token Passing with Task Swaps

**Paper:** Ma, Li, Kumar, Koenig, "Lifelong Multi-Agent Path Finding for Online Pickup and Delivery Tasks", AAMAS 2017.
**arXiv:** [1705.10868](https://arxiv.org/abs/1705.10868)
**No official reference impl found.** Closest: [github.com/Lodz97/Multi-Agent_Pickup_and_Delivery](https://github.com/Lodz97/Multi-Agent_Pickup_and_Delivery) (Python, TP with recovery)

### Algorithm 2 (TPTS) — Key Lines
- **Line 7:** `GetTask(ai, token)` — recursive task assignment
- **Lines 20-33:** Swap mechanism:
  - Line 21: Snapshot token, task set, assignments
  - Line 22: Identify currently assigned agent ai'
  - Line 23: Tentatively unassign ai' and assign ai
  - Line 26: Compare arrival times (A* spacetime cost)
  - Line 27: If ai reaches pickup faster → recursive `GetTask(ai', token)`
  - Line 33: On failure, restore snapshot
- **Path1:** collision-free A* path from current pos through pickup to delivery
- **Path2:** deadlock avoidance — idle agents move to non-task endpoints

### Key Guarantees
- **Theorem 3:** All well-formed MAPD instances are solvable, and TP solves them.
- **Property 4:** GetTask returns successfully for well-formed instances.
- **Theorem 5:** TPTS solves all well-formed MAPD instances.
- **Well-formedness (Definition 1):** finite tasks, at least m+1 endpoints, path between any two endpoints traversing no other endpoints.

### MAFIS Deviations (documented)
1. No recursive GetTask (MAFIS scheduler owns task assignment)
2. No Path2 endpoint parking (MAFIS has no designated parking spots)
3. Swap cooldown is our addition (prevents oscillation without recursion)

---

## PIBT+APF — PIBT with Artificial Potential Fields

**Paper:** Pertzovsky, Stern, Felner, Zivan, "Enhancing Lifelong Multi-Agent Path-finding by Using Artificial Potential Fields", 2025.
**arXiv:** [2505.22753](https://arxiv.org/abs/2505.22753)
**Reference impl:** [github.com/Arseni1919/APFs_for_MAPF_Implementation_v2](https://github.com/Arseni1919/APFs_for_MAPF_Implementation_v2) (Python)

### APF Construction
- **Equation 4 (Temporal APF):** `APF_i(v, t) = w * gamma^(-dist)` for `dist <= d_max`, 0 otherwise
- **Equation 11 (Per-agent aggregated):** `PIBT_APF_i(v) = SUM over t in {0..t_max} of APF_i(v, t)`
- **Equation 12 (Total at candidate):** `costAPF(v) = SUM over i in {1..k-1} of PIBT_APF_i(v)`
- **Neighbor ranking:** `sort by h(v) + costAPF(v)` (ascending). Goal cell always returns 0.

### Critical Integration Detail
APF is updated **sequentially inside the PIBT recursion** — after agent k commits its next position, its projected path APF is added to the shared field. Agent k+1 sees the accumulated field. This is NOT a pre-computed static layer.

### Recommended Parameters (Table 1)
| Parameter | PIBT+APF Value |
|-----------|---------------|
| w (weight) | 0.1 |
| d_max (radius) | 2 |
| gamma (decay) | 3 |
| t_max (lookahead) | 2 |

### Key Results
- LMAPF: ~20% average throughput improvement over vanilla PIBT
- One-shot MAPF: APF does NOT help (soft guidance insufficient for single-solve)
- `w` is highly sensitive: 0.1 >> 1.0 for PIBT+APF
