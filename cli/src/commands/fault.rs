//! `mafis fault run --fault-config <path.toml>` — TOML-driven fault experiment.
//!
//! The TOML file describes a single fault scenario and the experiment
//! parameters (solver, topology, agent count, seed range, tick count).
//! The CLI validates the file, then shells out to:
//!
//!   cargo test --release --test experiment_suite fault_from_env -- --nocapture
//!
//! Fault parameters are passed as `MAFIS_FAULT_*` environment variables so
//! they cross the process boundary without requiring the CLI crate to depend
//! on the `mafis` library.

use std::path::Path;

use serde::Deserialize;

use crate::shell;
use crate::style;

// ---------------------------------------------------------------------------
// TOML schema
// ---------------------------------------------------------------------------

/// Top-level TOML config loaded from `--fault-config <path>`.
///
/// All fields are optional and fall back to documented defaults so that
/// minimal files are valid:
///
/// ```toml
/// [scenario]
/// type = "burst_failure"
/// burst_kill_percent = 20.0
/// burst_at_tick = 100
/// ```
#[derive(Debug, Deserialize)]
pub struct FaultTomlConfig {
    #[serde(default)]
    pub scenario: FaultScenarioToml,

    #[serde(default)]
    pub experiment: ExperimentParamsToml,
}

/// Fault scenario section `[scenario]`.
///
/// `type` selects which set of fields is active.  Unknown keys for the
/// inactive scenario types are ignored — all fields are always present
/// in the struct with defaults so partial files are accepted.
#[derive(Debug, Deserialize)]
pub struct FaultScenarioToml {
    /// One of: `burst_failure`, `burst_50pct` (alias), `wear_medium` (alias),
    /// `wear_high` (alias), `wear_based`, `zone_outage`, `zone_50t` (alias),
    /// `intermittent`, `intermittent_fault` (alias).
    #[serde(rename = "type", default = "default_scenario_type")]
    pub scenario_type: String,

    // Burst Failure
    #[serde(default = "default_burst_kill_percent")]
    pub burst_kill_percent: f32,
    #[serde(default = "default_burst_at_tick")]
    pub burst_at_tick: u64,

    // Wear-Based
    /// `low` | `medium` | `high`
    #[serde(default = "default_wear_rate")]
    pub wear_heat_rate: String,
    /// Optional custom Weibull beta, overrides `wear_heat_rate` preset.
    pub weibull_beta: Option<f32>,
    /// Optional custom Weibull eta, overrides `wear_heat_rate` preset.
    pub weibull_eta: Option<f32>,

    // Zone Outage
    #[serde(default = "default_zone_at_tick")]
    pub zone_at_tick: u64,
    #[serde(default = "default_zone_latency_duration")]
    pub zone_latency_duration: u32,

    // Intermittent Fault
    #[serde(default = "default_intermittent_mtbf_ticks")]
    pub intermittent_mtbf_ticks: u64,
    #[serde(default = "default_intermittent_recovery_ticks")]
    pub intermittent_recovery_ticks: u32,
    #[serde(default)]
    pub intermittent_start_tick: u64,
}

impl Default for FaultScenarioToml {
    fn default() -> Self {
        Self {
            scenario_type: default_scenario_type(),
            burst_kill_percent: default_burst_kill_percent(),
            burst_at_tick: default_burst_at_tick(),
            wear_heat_rate: default_wear_rate(),
            weibull_beta: None,
            weibull_eta: None,
            zone_at_tick: default_zone_at_tick(),
            zone_latency_duration: default_zone_latency_duration(),
            intermittent_mtbf_ticks: default_intermittent_mtbf_ticks(),
            intermittent_recovery_ticks: default_intermittent_recovery_ticks(),
            intermittent_start_tick: 0,
        }
    }
}

fn default_scenario_type() -> String {
    "burst_failure".into()
}
fn default_burst_kill_percent() -> f32 {
    20.0
}
fn default_burst_at_tick() -> u64 {
    100
}
fn default_wear_rate() -> String {
    "medium".into()
}
fn default_zone_at_tick() -> u64 {
    100
}
fn default_zone_latency_duration() -> u32 {
    50
}
fn default_intermittent_mtbf_ticks() -> u64 {
    80
}
fn default_intermittent_recovery_ticks() -> u32 {
    15
}

/// Experiment parameters section `[experiment]`.
#[derive(Debug, Deserialize)]
pub struct ExperimentParamsToml {
    /// Solver to use: `pibt` | `rhcr_pbs` | `token_passing`
    #[serde(default = "default_solver")]
    pub solver: String,
    /// Topology name (must match a topology JSON in `topologies/`)
    #[serde(default = "default_topology")]
    pub topology: String,
    /// Task scheduler: `random` | `closest`
    #[serde(default = "default_scheduler")]
    pub scheduler: String,
    /// Number of agents (must match topology's required agent count)
    #[serde(default = "default_num_agents")]
    pub num_agents: usize,
    /// Simulation length in ticks
    #[serde(default = "default_tick_count")]
    pub tick_count: u64,
    /// Seeds to run (comma-separated in TOML, e.g. `seeds = [42, 123, 456]`)
    #[serde(default = "default_seeds")]
    pub seeds: Vec<u64>,
}

impl Default for ExperimentParamsToml {
    fn default() -> Self {
        Self {
            solver: default_solver(),
            topology: default_topology(),
            scheduler: default_scheduler(),
            num_agents: default_num_agents(),
            tick_count: default_tick_count(),
            seeds: default_seeds(),
        }
    }
}

fn default_solver() -> String {
    "pibt".into()
}
fn default_topology() -> String {
    "warehouse_single_dock".into()
}
fn default_scheduler() -> String {
    "random".into()
}
fn default_num_agents() -> usize {
    40
}
fn default_tick_count() -> u64 {
    500
}
fn default_seeds() -> Vec<u64> {
    vec![42, 123, 456]
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Known canonical type ids and their shorthand aliases.
const KNOWN_TYPES: &[(&str, &str)] = &[
    ("burst_failure", "burst_failure"),
    ("burst_20pct", "burst_failure"),
    ("burst_50pct", "burst_failure"),
    ("wear_based", "wear_based"),
    ("wear_medium", "wear_based"),
    ("wear_high", "wear_based"),
    ("zone_outage", "zone_outage"),
    ("zone_50t", "zone_outage"),
    ("intermittent_fault", "intermittent_fault"),
    ("intermittent", "intermittent_fault"),
];

/// Resolve a user-supplied type string to the canonical id used by the runner.
fn canonical_type(s: &str) -> Option<&'static str> {
    KNOWN_TYPES.iter().find(|(alias, _)| *alias == s).map(|(_, canon)| *canon)
}

fn validate(cfg: &FaultTomlConfig) -> anyhow::Result<()> {
    let t = cfg.scenario.scenario_type.as_str();
    if canonical_type(t).is_none() {
        let known: Vec<&str> = KNOWN_TYPES.iter().map(|(a, _)| *a).collect();
        anyhow::bail!("Unknown scenario type '{}'. Known types: {}", t, known.join(", "));
    }

    let wear = cfg.scenario.wear_heat_rate.as_str();
    if !["low", "medium", "high"].contains(&wear) {
        anyhow::bail!("Unknown wear_heat_rate '{}'. Must be one of: low, medium, high", wear);
    }

    let solver = cfg.experiment.solver.as_str();
    if !["pibt", "rhcr_pbs", "token_passing"].contains(&solver) {
        anyhow::bail!("Unknown solver '{}'. Must be one of: pibt, rhcr_pbs, token_passing", solver);
    }

    let scheduler = cfg.experiment.scheduler.as_str();
    if !["random", "closest"].contains(&scheduler) {
        anyhow::bail!("Unknown scheduler '{}'. Must be one of: random, closest", scheduler);
    }

    if cfg.experiment.seeds.is_empty() {
        anyhow::bail!("experiment.seeds must contain at least one seed");
    }

    if cfg.experiment.num_agents == 0 {
        anyhow::bail!("experiment.num_agents must be > 0");
    }

    if cfg.experiment.tick_count == 0 {
        anyhow::bail!("experiment.tick_count must be > 0");
    }

    if cfg.scenario.burst_kill_percent <= 0.0 || cfg.scenario.burst_kill_percent > 100.0 {
        anyhow::bail!(
            "scenario.burst_kill_percent must be in (0, 100], got {}",
            cfg.scenario.burst_kill_percent
        );
    }

    // Weibull custom params: either both or neither
    match (cfg.scenario.weibull_beta, cfg.scenario.weibull_eta) {
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!(
                "scenario.weibull_beta and scenario.weibull_eta must both be set \
                 or both be omitted"
            );
        }
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Env-var encoding
// ---------------------------------------------------------------------------

/// Encode all fault+experiment parameters as `MAFIS_FAULT_*` environment variables
/// so they can be read by the `fault_from_env` integration test on the other side
/// of the `cargo test` process boundary.
fn build_env_vars(cfg: &FaultTomlConfig) -> Vec<(String, String)> {
    let s = &cfg.scenario;
    let e = &cfg.experiment;

    let scenario_type = canonical_type(&s.scenario_type).unwrap_or("burst_failure");

    let seeds_str: String = e.seeds.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");

    let mut vars = vec![
        ("MAFIS_FAULT_TYPE".into(), scenario_type.into()),
        ("MAFIS_FAULT_BURST_KILL_PERCENT".into(), s.burst_kill_percent.to_string()),
        ("MAFIS_FAULT_BURST_AT_TICK".into(), s.burst_at_tick.to_string()),
        ("MAFIS_FAULT_WEAR_HEAT_RATE".into(), s.wear_heat_rate.clone()),
        ("MAFIS_FAULT_ZONE_AT_TICK".into(), s.zone_at_tick.to_string()),
        ("MAFIS_FAULT_ZONE_LATENCY_DURATION".into(), s.zone_latency_duration.to_string()),
        ("MAFIS_FAULT_INTERMITTENT_MTBF_TICKS".into(), s.intermittent_mtbf_ticks.to_string()),
        (
            "MAFIS_FAULT_INTERMITTENT_RECOVERY_TICKS".into(),
            s.intermittent_recovery_ticks.to_string(),
        ),
        ("MAFIS_FAULT_INTERMITTENT_START_TICK".into(), s.intermittent_start_tick.to_string()),
        ("MAFIS_FAULT_SOLVER".into(), e.solver.clone()),
        ("MAFIS_FAULT_TOPOLOGY".into(), e.topology.clone()),
        ("MAFIS_FAULT_SCHEDULER".into(), e.scheduler.clone()),
        ("MAFIS_FAULT_NUM_AGENTS".into(), e.num_agents.to_string()),
        ("MAFIS_FAULT_TICK_COUNT".into(), e.tick_count.to_string()),
        ("MAFIS_FAULT_SEEDS".into(), seeds_str),
    ];

    if let Some(beta) = s.weibull_beta {
        vars.push(("MAFIS_FAULT_WEIBULL_BETA".into(), beta.to_string()));
    }
    if let Some(eta) = s.weibull_eta {
        vars.push(("MAFIS_FAULT_WEIBULL_ETA".into(), eta.to_string()));
    }

    vars
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Load and print the parsed config for inspection — does not run anything.
pub fn validate_config(path: &Path) -> anyhow::Result<()> {
    let cfg = load_config(path)?;

    println!("{}", style::section("Fault Config"));
    println!("  file:     {}", path.display());
    println!("  type:     {}", cfg.scenario.scenario_type);

    match canonical_type(&cfg.scenario.scenario_type).unwrap_or("?") {
        "burst_failure" => {
            println!(
                "  kill:     {}%  at tick {}",
                cfg.scenario.burst_kill_percent, cfg.scenario.burst_at_tick
            );
        }
        "wear_based" => {
            if let (Some(b), Some(e)) = (cfg.scenario.weibull_beta, cfg.scenario.weibull_eta) {
                println!("  weibull:  beta={b}  eta={e}  (custom override)");
            } else {
                println!("  wear:     {}", cfg.scenario.wear_heat_rate);
            }
        }
        "zone_outage" => {
            println!(
                "  zone:     at tick {}  duration {}t",
                cfg.scenario.zone_at_tick, cfg.scenario.zone_latency_duration
            );
        }
        "intermittent_fault" => {
            println!(
                "  intermit: mtbf={}t  recovery={}t  start_tick={}",
                cfg.scenario.intermittent_mtbf_ticks,
                cfg.scenario.intermittent_recovery_ticks,
                cfg.scenario.intermittent_start_tick,
            );
        }
        _ => {}
    }

    println!();
    println!("  solver:   {}", cfg.experiment.solver);
    println!("  topology: {}", cfg.experiment.topology);
    println!("  agents:   {}", cfg.experiment.num_agents);
    println!("  ticks:    {}", cfg.experiment.tick_count);
    println!("  seeds:    {} runs", cfg.experiment.seeds.len());
    println!();
    style::print_success("Config valid.");
    Ok(())
}

/// Load TOML config, validate it, then shell out to the experiment runner.
pub fn run_with_config(root: &Path, path: &Path) -> anyhow::Result<()> {
    let cfg = load_config(path)?;

    println!("{}", style::section("Fault Config Run"));
    println!("  config:   {}", path.display());
    println!("  type:     {}", cfg.scenario.scenario_type);
    println!("  solver:   {}", cfg.experiment.solver);
    println!("  topology: {}", cfg.experiment.topology);
    println!("  agents:   {}", cfg.experiment.num_agents);
    println!("  ticks:    {}", cfg.experiment.tick_count);
    println!("  seeds:    {} runs", cfg.experiment.seeds.len());
    println!();

    let env_vars = build_env_vars(&cfg);

    // Build cargo test argv
    let args =
        &["test", "--release", "--test", "experiment_suite", "fault_from_env", "--", "--nocapture"];

    let status = shell::run_streaming_with_env("cargo", args, root, &env_vars)?;

    if !status.success() {
        anyhow::bail!("fault experiment failed (see output above)");
    }

    style::print_success("Fault experiment complete. Results in results/");
    Ok(())
}

fn load_config(path: &Path) -> anyhow::Result<FaultTomlConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", path.display(), e))?;

    let cfg: FaultTomlConfig = toml::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("TOML parse error in '{}': {}", path.display(), e))?;

    validate(&cfg)?;
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> anyhow::Result<FaultTomlConfig> {
        let cfg: FaultTomlConfig = toml::from_str(toml_str).map_err(|e| anyhow::anyhow!("{e}"))?;
        validate(&cfg)?;
        Ok(cfg)
    }

    #[test]
    fn minimal_burst_file_is_valid() {
        let cfg = parse(
            r#"
[scenario]
type = "burst_failure"
"#,
        )
        .unwrap();
        assert_eq!(cfg.scenario.burst_kill_percent, 20.0);
        assert_eq!(cfg.scenario.burst_at_tick, 100);
    }

    #[test]
    fn wear_based_with_preset() {
        let cfg = parse(
            r#"
[scenario]
type = "wear_based"
wear_heat_rate = "high"
"#,
        )
        .unwrap();
        assert_eq!(cfg.scenario.wear_heat_rate, "high");
        assert!(cfg.scenario.weibull_beta.is_none());
    }

    #[test]
    fn wear_based_with_custom_weibull() {
        let cfg = parse(
            r#"
[scenario]
type = "wear_based"
weibull_beta = 3.5
weibull_eta  = 150.0
"#,
        )
        .unwrap();
        assert_eq!(cfg.scenario.weibull_beta, Some(3.5));
        assert_eq!(cfg.scenario.weibull_eta, Some(150.0));
    }

    #[test]
    fn partial_weibull_is_rejected() {
        let result = parse(
            r#"
[scenario]
type = "wear_based"
weibull_beta = 3.5
"#,
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("both be set or both be omitted"), "msg: {msg}");
    }

    #[test]
    fn zone_alias_resolves() {
        let cfg = parse(
            r#"
[scenario]
type = "zone_50t"
"#,
        )
        .unwrap();
        assert_eq!(canonical_type(&cfg.scenario.scenario_type), Some("zone_outage"));
    }

    #[test]
    fn intermittent_alias_resolves() {
        let cfg = parse(
            r#"
[scenario]
type = "intermittent"
"#,
        )
        .unwrap();
        assert_eq!(canonical_type(&cfg.scenario.scenario_type), Some("intermittent_fault"));
    }

    #[test]
    fn burst_50pct_alias_resolves() {
        let cfg = parse(
            r#"
[scenario]
type = "burst_50pct"
burst_kill_percent = 50.0
"#,
        )
        .unwrap();
        assert_eq!(canonical_type(&cfg.scenario.scenario_type), Some("burst_failure"));
    }

    #[test]
    fn unknown_type_is_rejected() {
        let result = parse(
            r#"
[scenario]
type = "nuclear_meltdown"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn unknown_solver_is_rejected() {
        let result = parse(
            r#"
[scenario]
type = "burst_failure"

[experiment]
solver = "cbs"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn empty_seeds_is_rejected() {
        let result = parse(
            r#"
[scenario]
type = "burst_failure"

[experiment]
seeds = []
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn env_vars_contain_all_keys() {
        let cfg = parse(
            r#"
[scenario]
type = "burst_failure"
burst_kill_percent = 20.0
burst_at_tick = 100

[experiment]
solver = "pibt"
topology = "warehouse_single_dock"
seeds = [42, 123]
"#,
        )
        .unwrap();
        let vars = build_env_vars(&cfg);
        let keys: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
        for required in &[
            "MAFIS_FAULT_TYPE",
            "MAFIS_FAULT_SOLVER",
            "MAFIS_FAULT_TOPOLOGY",
            "MAFIS_FAULT_SEEDS",
            "MAFIS_FAULT_NUM_AGENTS",
            "MAFIS_FAULT_TICK_COUNT",
        ] {
            assert!(keys.contains(required), "missing env var {required}");
        }
    }

    #[test]
    fn env_var_seeds_encoded_as_csv() {
        let cfg = parse(
            r#"
[scenario]
type = "burst_failure"

[experiment]
seeds = [42, 123, 456]
"#,
        )
        .unwrap();
        let vars = build_env_vars(&cfg);
        let seeds = vars.iter().find(|(k, _)| k == "MAFIS_FAULT_SEEDS").unwrap();
        assert_eq!(seeds.1, "42,123,456");
    }
}
