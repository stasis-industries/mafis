//! One-shot probe: run the exact config the user reported (pibt /
//! warehouse_medium / burst_20pct / random / 10 agents / seed 456 / 500
//! ticks) through the experiment pipeline and print CT + summary so we can
//! compare against the observatory's reported CT=0.858.
//!
//! Run: `cargo test --release --test ct_audit_probe -- --nocapture`

use mafis::experiment::config::ExperimentConfig;
use mafis::experiment::runner::run_single_experiment;
use mafis::fault::scenario::{FaultScenario, FaultScenarioType};

#[test]
fn ct_probe_pibt_warehouse_medium_burst20_random_10a_seed456() {
    let scenario = FaultScenario {
        enabled: true,
        scenario_type: FaultScenarioType::BurstFailure,
        burst_kill_percent: 20.0,
        burst_at_tick: 100,
        ..Default::default()
    };
    let config = ExperimentConfig {
        solver_name: "pibt".into(),
        topology_name: "warehouse_medium".into(),
        scenario: Some(scenario),
        scheduler_name: "random".into(),
        num_agents: 10,
        seed: 456,
        tick_count: 500,
        custom_map: None,
        rhcr_override: None,
    };
    let r = run_single_experiment(&config);
    let b = &r.baseline_metrics;
    let f = &r.faulted_metrics;
    eprintln!("=== CT AUDIT PROBE ===");
    eprintln!("  baseline_tasks={}  baseline_tp={:.4}", b.total_tasks, b.avg_throughput);
    eprintln!("  faulted_tasks ={}  faulted_tp ={:.4}", f.total_tasks, f.avg_throughput);
    eprintln!("  survival_rate ={:.4}", f.survival_rate);
    eprintln!("  fault_tolerance={:.4}", f.fault_tolerance);
    eprintln!("  critical_time ={:.4}  (observatory reported 0.858)", f.critical_time);
    eprintln!("  itae          ={:.2}", f.itae);
    eprintln!("  rapidity      ={:.4}", f.rapidity);
    eprintln!("  attack_rate   ={:.4}", f.attack_rate);
    // Sanity bounds — not the actual parity check, just guards.
    assert!(f.critical_time >= 0.0 && f.critical_time <= 1.0);
}
