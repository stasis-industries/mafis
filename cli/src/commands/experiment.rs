use std::path::Path;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::Table;
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
        runs: 75,
        description: "3 solvers \u{00d7} 5 scenarios \u{00d7} 5 seeds",
        test_fn: "solver_resilience",
    },
    ExperimentInfo {
        name: "scale_sensitivity",
        runs: 100,
        description: "4 agent counts \u{00d7} 5 scenarios \u{00d7} 5 seeds",
        test_fn: "scale_sensitivity",
    },
    ExperimentInfo {
        name: "scheduler_effect",
        runs: 50,
        description: "2 schedulers \u{00d7} 5 scenarios \u{00d7} 5 seeds",
        test_fn: "scheduler_effect",
    },
    ExperimentInfo {
        name: "topology_medium",
        runs: 25,
        description: "warehouse_large, 40 agents, 5 scenarios \u{00d7} 5 seeds",
        test_fn: "topology_medium",
    },
    ExperimentInfo {
        name: "topology_large",
        runs: 25,
        description: "warehouse_large, 100 agents, 5 scenarios \u{00d7} 5 seeds",
        test_fn: "topology_large",
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
    table.add_row(vec![
        "TOTAL".bold().to_string(),
        total.bold().to_string(),
        String::new(),
    ]);

    println!("{table}");
    println!();
    println!(
        "  Run with: {}",
        style::info("experiment run <name>")
    );
    println!(
        "  Smoke test: {}",
        style::info("experiment smoke")
    );
    println!(
        "  Full suite: {}",
        style::info("experiment run-all")
    );
    Ok(())
}

pub fn run(root: &Path, name: &str) -> anyhow::Result<()> {
    let exp = EXPERIMENTS
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| {
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
        &[
            "test",
            "--test",
            "paper_experiments",
            "paper_smoke",
            "--",
            "--nocapture",
        ],
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
