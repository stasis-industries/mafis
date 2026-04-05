use std::path::Path;

use comfy_table::Table;
use comfy_table::presets::UTF8_FULL_CONDENSED;
use owo_colors::OwoColorize;

use crate::shell;
use crate::style;

struct ExperimentInfo {
    name: &'static str,
    runs: usize,
    description: &'static str,
    test_fn: &'static str,
}

const EXPERIMENTS: &[ExperimentInfo] = &[
    ExperimentInfo {
        name: "solver_resilience",
        runs: 1260,
        description: "6 solvers \u{00d7} 7 scenarios \u{00d7} 30 seeds",
        test_fn: "solver_resilience",
    },
    ExperimentInfo {
        name: "scale_sensitivity",
        runs: 840,
        description: "4 agent counts \u{00d7} 7 scenarios \u{00d7} 30 seeds",
        test_fn: "scale_sensitivity",
    },
    ExperimentInfo {
        name: "scheduler_effect",
        runs: 420,
        description: "2 schedulers \u{00d7} 7 scenarios \u{00d7} 30 seeds",
        test_fn: "scheduler_effect",
    },
    ExperimentInfo {
        name: "topology_medium",
        runs: 210,
        description: "warehouse_large, 40 agents, 7 scenarios \u{00d7} 30 seeds",
        test_fn: "topology_medium",
    },
    ExperimentInfo {
        name: "topology_large",
        runs: 210,
        description: "kiva_warehouse, 80 agents, 7 scenarios \u{00d7} 30 seeds",
        test_fn: "topology_large",
    },
    ExperimentInfo {
        name: "braess_resilience",
        runs: 9800,
        description: "7 solvers \u{00d7} 4 densities \u{00d7} 7 scenarios \u{00d7} 50 seeds",
        test_fn: "braess_resilience",
    },
    ExperimentInfo {
        name: "paams_full",
        runs: 16170,
        description: "PAAMS 2026: 7 solvers \u{00d7} 3 topologies \u{00d7} 7 scenarios \u{00d7} 30 seeds",
        test_fn: "paams_full",
    },
];

pub fn list() -> anyhow::Result<()> {
    println!("{}", style::section("Experiments"));
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["Experiment", "Runs", "Variables"]);

    for exp in EXPERIMENTS {
        table.add_row(vec![
            exp.name.to_string(),
            exp.runs.to_string(),
            exp.description.to_string(),
        ]);
    }

    let total: usize = EXPERIMENTS.iter().map(|e| e.runs).sum();
    table.add_row(vec!["TOTAL".bold().to_string(), total.bold().to_string(), String::new()]);

    println!("{table}");
    println!();
    println!("  Run with: {}", style::info("experiment run <name>"));
    println!("  Smoke test: {}", style::info("experiment smoke"));
    println!("  Full suite: {}", style::info("experiment run-all"));
    Ok(())
}

pub fn run(root: &Path, name: &str) -> anyhow::Result<()> {
    let exp = EXPERIMENTS.iter().find(|e| e.name == name).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown experiment '{}'. Run 'experiment list' to see available experiments.",
            name
        )
    })?;

    println!("{}", style::section(&format!("Experiment: {}", exp.name)));
    println!("  {} runs  {}", exp.runs, style::dim(exp.description));
    println!();

    let status = shell::run_streaming(
        "cargo",
        &[
            "test",
            "--release",
            "--test",
            "paper_experiments",
            exp.test_fn,
            "--",
            "--ignored",
            "--nocapture",
        ],
        root,
    )?;

    if !status.success() {
        anyhow::bail!("experiment '{}' failed", name);
    }

    style::print_success(&format!("Experiment '{}' complete. Results in results/", name));
    Ok(())
}

pub fn smoke(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Smoke Test"));
    println!("  Quick validation (2 runs, ~1s)");
    println!();

    let status = shell::run_streaming(
        "cargo",
        &["test", "--test", "paper_experiments", "paper_smoke", "--", "--nocapture"],
        root,
    )?;

    if !status.success() {
        anyhow::bail!("smoke test failed");
    }

    style::print_success("Smoke test passed.");
    Ok(())
}

pub fn run_all(root: &Path) -> anyhow::Result<()> {
    let total: usize = EXPERIMENTS.iter().map(|e| e.runs).sum();
    println!("{}", style::section("Full Paper Suite"));
    println!("  {} total runs across {} experiments", total, EXPERIMENTS.len());
    println!();

    let status = shell::run_streaming(
        "cargo",
        &[
            "test",
            "--release",
            "--test",
            "paper_experiments",
            "full_paper_matrix",
            "--",
            "--ignored",
            "--nocapture",
        ],
        root,
    )?;

    if !status.success() {
        anyhow::bail!("full paper matrix failed");
    }

    style::print_success("All experiments complete. Results in results/");
    Ok(())
}
