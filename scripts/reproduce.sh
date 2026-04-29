#!/bin/sh
# reproduce.sh — Full reproduction pipeline for MAFIS experiment results.
# Usage: sh scripts/reproduce.sh
#
# Runs: type check, test suite, calibration, experiments, analysis.
# Braess analysis instructions are printed at the end.
#
# Expected total time: ~1-2 hours on 8+ cores.

set -e

echo "==========================================================="
echo " MAFIS Reproduction Pipeline"
echo "==========================================================="
echo ""

# ---------------------------------------------------------------------------
# Step 1: Type check
# ---------------------------------------------------------------------------
echo "-- Step 1/5: Type check --"
cargo check
echo "[OK] Type check passed"
echo ""

# ---------------------------------------------------------------------------
# Step 2: Full test suite
# ---------------------------------------------------------------------------
echo "-- Step 2/5: Test suite --"
cargo test --release
echo "[OK] Tests passed"
echo ""

# ---------------------------------------------------------------------------
# Step 3: Calibration / property verification
# ---------------------------------------------------------------------------
echo "-- Step 3/5: Calibration (property verification) --"
cargo test --release --test calibration -- --nocapture
echo "[OK] Calibration passed"
echo ""

# ---------------------------------------------------------------------------
# Step 4: Full experiment matrix (~1-2 hours)
#   solver_resilience  — 3 solvers × 6 scenarios × 30 seeds (540 configs)
#   scale_sensitivity  — 4 densities × 6 scenarios × 30 seeds (720 configs)
#   scheduler_effect   — 2 schedulers × 6 scenarios × 30 seeds (360 configs)
#   topology_effect    — multiple topologies × 6 scenarios × 30 seeds
# ---------------------------------------------------------------------------
echo "-- Step 4/5: Full experiment matrix (~1-2 hours) --"
cargo test --release full_legacy_matrix -- --ignored --nocapture
echo "[OK] Experiments complete"
echo ""

# ---------------------------------------------------------------------------
# Step 5: Statistical analysis
# ---------------------------------------------------------------------------
echo "-- Step 5/5: Statistical analysis --"
python3 analysis/solver_resilience_analysis.py
python3 analysis/scale_sensitivity_analysis.py
python3 analysis/scheduler_effect_analysis.py
python3 analysis/topology_effect_analysis.py
echo "[OK] Analysis complete"
echo ""

echo "==========================================================="
echo " Done. Results written to results/"
echo "==========================================================="
ls -la results/*.csv results/*.json 2>/dev/null | head -40
echo ""

# ---------------------------------------------------------------------------
# Optional: Braess paradox experiment (separate, ~2-4 additional hours)
# ---------------------------------------------------------------------------
echo "Optional — Braess paradox analysis:"
echo "  # Main 6-scenario experiment (3 solvers × 4 densities × 6 scenarios × 50 seeds):"
echo "  cargo test --release braess_resilience -- --ignored --nocapture"
echo ""
echo "  # Category 3 complement (permanent zone outage):"
echo "  cargo test --release run_braess_perm_zone -- --ignored --nocapture"
echo ""
echo "  # Analysis (reads both CSVs if present):"
echo "  python3 analysis/braess_analysis.py"
echo ""
echo "Optional — Cross-topology validation (480 runs):"
echo "  cargo test --release run_cross_topology -- --ignored --nocapture"
echo ""
echo "Optional — New solver resilience variant (closest scheduler, n=20, 540 runs):"
echo "  cargo test --release run_new_solver_resilience -- --ignored --nocapture"
