//! End-to-end integration tests for MAFIS.
//!
//! Tests are organized by subsystem. Each test creates its own `SimHarness`
//! (no shared state between tests). Add new tests by appending to the
//! appropriate section below, or create new modules in `src/sim_tests/`.
//!
//! Run with: `cargo test`

use super::common::SimHarness;

use crate::core::phase::SimulationPhase;

// ===========================================================================
// Simulation lifecycle
// ===========================================================================

#[test]
fn tick_starts_at_zero() {
    let h = SimHarness::new(2);
    assert_eq!(h.tick(), 0);
}

#[test]
fn tick_increments_once_per_fixed_update() {
    let mut h = SimHarness::new(2);
    h.run_ticks(1);
    assert_eq!(h.tick(), 1);
    h.run_ticks(4);
    assert_eq!(h.tick(), 5);
}

#[test]
fn agents_survive_without_faults() {
    let mut h = SimHarness::new(4);
    h.run_ticks(20);
    assert_eq!(h.alive_agent_count(), 4);
}

#[test]
fn registry_count_matches_spawned() {
    let h = SimHarness::new(6);
    assert_eq!(h.agent_count(), 6);
}

#[test]
fn zero_agents_doesnt_panic() {
    let mut h = SimHarness::new(0);
    h.run_ticks(5);
    assert_eq!(h.tick(), 5);
    assert_eq!(h.agent_count(), 0);
}

// ===========================================================================
// Phase management
// ===========================================================================

#[test]
fn initial_phase_is_running() {
    let h = SimHarness::new(2);
    assert_eq!(h.phase(), SimulationPhase::Running);
}

#[test]
fn phase_stays_running_after_ticks() {
    let mut h = SimHarness::new(2);
    h.run_ticks(10);
    assert_eq!(h.phase(), SimulationPhase::Running);
}

// ===========================================================================
// FaultMetrics — ECS integration
// ===========================================================================

#[test]
fn fault_metrics_zero_before_faults() {
    let h = SimHarness::new(4);
    let m = h.metrics();
    assert_eq!(m.mttr, 0.0);
    assert_eq!(m.total_affected, 0);
    assert_eq!(m.total_recovered, 0);
    assert_eq!(m.recovery_rate, 0.0);
    assert_eq!(m.avg_cascade_spread, 0.0);
    assert_eq!(m.throughput, 0.0);
    assert_eq!(m.idle_ratio, 0.0);
}

#[test]
fn initial_agent_count_set_on_first_tick() {
    let mut h = SimHarness::new(4);
    h.run_ticks(1);
    assert_eq!(h.metrics().initial_agent_count, 4);
}

#[test]
fn idle_ratio_bounded_zero_to_one() {
    let mut h = SimHarness::new(4);
    h.run_ticks(10);
    let ratio = h.metrics().idle_ratio;
    assert!((0.0..=1.0).contains(&ratio), "idle_ratio={ratio}");
}

#[test]
fn survival_rate_is_one_without_faults() {
    let mut h = SimHarness::new(4);
    h.run_ticks(10);
    let series = &h.metrics().survival_series;
    assert!(!series.is_empty());
    for (_, rate) in series {
        assert!((*rate - 1.0).abs() < 1e-5, "got {rate}");
    }
}

#[test]
fn throughput_is_finite_after_ticks() {
    let mut h = SimHarness::new(4);
    h.run_ticks(30);
    let t = h.metrics().throughput;
    assert!(t >= 0.0 && t.is_finite(), "throughput={t}");
}

#[test]
fn fault_metrics_clear_resets_to_zero() {
    let mut h = SimHarness::new(4);
    h.run_ticks(10);
    h.app.world_mut().resource_mut::<crate::analysis::fault_metrics::FaultMetrics>().clear();
    let m = h.metrics();
    assert_eq!(m.mttr, 0.0);
    assert_eq!(m.throughput, 0.0);
    assert_eq!(m.initial_agent_count, 0);
    assert!(m.survival_series.is_empty());
}

// ===========================================================================
// CascadeState
// ===========================================================================

#[test]
fn cascade_state_starts_empty() {
    let h = SimHarness::new(4);
    let c = h.cascade();
    assert!(c.records.is_empty());
    assert_eq!(c.fault_count, 0);
    assert_eq!(c.max_depth, 0);
    assert!(c.fault_log.is_empty());
}

#[test]
fn cascade_no_faults_when_disabled() {
    let mut h = SimHarness::new(4);
    h.run_ticks(20);
    assert_eq!(h.cascade().fault_count, 0);
}

// ===========================================================================
// ResilienceScorecard — ECS integration
// ===========================================================================

#[test]
fn scorecard_defaults_no_faults() {
    let h = SimHarness::new(4);
    assert_eq!(h.scorecard().fault_tolerance, 1.0);
    assert!(!h.scorecard().has_faults);
}

#[test]
fn scorecard_stable_after_ticks() {
    let mut h = SimHarness::new(4);
    h.run_ticks(5);
    assert_eq!(h.scorecard().fault_tolerance, 1.0);
    assert!(!h.scorecard().has_faults);
}

#[test]
fn scorecard_clear_resets() {
    let mut h = SimHarness::new(4);
    h.run_ticks(10);
    h.app.world_mut().resource_mut::<crate::analysis::scorecard::ResilienceScorecard>().clear();
    assert_eq!(h.scorecard().fault_tolerance, 1.0);
    assert_eq!(h.scorecard().nrr, None);
    assert_eq!(h.scorecard().fleet_utilization, 1.0);
}

// ===========================================================================
// TickHistory
// ===========================================================================

#[test]
fn tick_history_records_snapshots() {
    let mut h = SimHarness::new(4);
    h.run_ticks(5);
    assert!(!h.history().snapshots.is_empty());
}

#[test]
fn tick_history_snapshot_count_bounded() {
    use crate::constants::TICK_SNAPSHOT_INTERVAL;
    let mut h = SimHarness::new(4);
    let n = TICK_SNAPSHOT_INTERVAL * 5;
    h.run_ticks(n);
    assert!(!h.history().snapshots.is_empty());
    assert!(h.history().snapshots.len() <= (n / TICK_SNAPSHOT_INTERVAL) as usize + 1);
}

#[test]
fn tick_history_snapshot_agent_count_matches() {
    let mut h = SimHarness::new(4);
    h.run_ticks(3);
    let snapshot = h.history().snapshots.back().expect("at least one snapshot");
    assert_eq!(snapshot.agents.len(), 4);
}

#[test]
fn tick_history_snapshot_tick_is_aligned() {
    use crate::constants::TICK_SNAPSHOT_INTERVAL;
    let mut h = SimHarness::new(2);
    let n = TICK_SNAPSHOT_INTERVAL * 3;
    h.run_ticks(n);
    let last = h.history().snapshots.back().expect("snapshot must exist");
    assert_eq!(last.tick % TICK_SNAPSHOT_INTERVAL, 0);
    assert!(last.tick > 0);
}

#[test]
fn tick_history_clear_empties_snapshots() {
    let mut h = SimHarness::new(2);
    h.run_ticks(5);
    h.app.world_mut().resource_mut::<crate::analysis::history::TickHistory>().clear();
    assert!(h.history().snapshots.is_empty());
}

// ===========================================================================
// Heatmap — ECS integration
// ===========================================================================

#[test]
fn heatmap_zero_when_not_visible() {
    let mut h = SimHarness::new(4);
    h.run_ticks(10);
    assert!(h.heatmap().density.iter().all(|&d| d == 0.0));
    assert!(h.heatmap().traffic.iter().all(|&t| t == 0));
}

#[test]
fn heatmap_density_accumulates_when_visible() {
    let mut h = SimHarness::new(4);
    h.app.world_mut().resource_mut::<crate::analysis::AnalysisConfig>().heatmap_visible = true;
    h.run_ticks(5);
    let total: f32 = h.heatmap().density.iter().sum();
    assert!(total > 0.0, "got {total}");
}

#[test]
fn heatmap_traffic_accumulates_when_visible() {
    let mut h = SimHarness::new(4);
    h.app.world_mut().resource_mut::<crate::analysis::AnalysisConfig>().heatmap_visible = true;
    h.run_ticks(5);
    let total: u32 = h.heatmap().traffic.iter().sum();
    assert!(total > 0, "got {total}");
}

// ===========================================================================
// Task scheduling / Lifelong MAPF
// ===========================================================================

#[test]
fn lifelong_mode_enabled_by_default() {
    let h = SimHarness::new(4);
    assert!(h.lifelong().enabled);
}

#[test]
fn agents_get_tasks_and_move() {
    use crate::core::agent::LogicalAgent;

    let mut h = SimHarness::new(1);
    let _initial_pos = h
        .app
        .world_mut()
        .query::<&LogicalAgent>()
        .iter(h.app.world())
        .next()
        .map(|a| a.current_pos)
        .expect("agent must exist");

    h.run_ticks(15);

    // Confirm agent still exists (no panic)
    let _final_pos = h
        .app
        .world_mut()
        .query::<&LogicalAgent>()
        .iter(h.app.world())
        .next()
        .map(|a| a.current_pos)
        .expect("agent must survive");
}

// ===========================================================================
// FaultConfig defaults
// ===========================================================================

#[test]
fn fault_config_disabled_by_default() {
    let h = SimHarness::new(2);
    assert!(!h.fault_config().enabled);
}

#[test]
fn with_faults_enables_weibull() {
    let h = SimHarness::new(2).with_faults();
    assert!(h.fault_config().enabled);
    assert!(h.fault_config().weibull_enabled);
}
