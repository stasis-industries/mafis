use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mafis",
    about = "MAFIS — Multi-Agent Fault Injection Simulator",
    version,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    /// Launch the desktop experiment runner
    Desktop {
        /// Run in debug mode (faster compile, slower runtime)
        #[arg(long)]
        debug: bool,
    },

    /// Run experiments (headless CLI)
    Experiment {
        #[command(subcommand)]
        action: ExperimentCommand,
    },

    /// View and compare results
    Results {
        #[command(subcommand)]
        action: ResultsCommand,
    },

    /// Run tests
    Test {
        /// Test name filter
        filter: Option<String>,
        /// Run in release mode
        #[arg(long)]
        release: bool,
    },

    /// Build + serve WASM observatory
    Serve {
        /// Skip build, serve existing artifacts
        #[arg(long)]
        no_build: bool,
        /// Port number (1-65535)
        #[arg(long, default_value = "4000", value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },

    /// Full WASM build pipeline
    Build {
        /// Native-only build (skip WASM)
        #[arg(long)]
        native: bool,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum ExperimentCommand {
    /// List all experiment presets
    List,
    /// Run a specific experiment
    Run {
        /// Experiment name (solver_resilience, scale_sensitivity, scheduler_effect, topology_medium, topology_large)
        name: String,
    },
    /// Quick smoke test (~1s)
    Smoke,
    /// Run all paper experiments
    RunAll,
}

#[derive(Subcommand, Clone, Debug)]
pub enum ResultsCommand {
    /// List result files
    List,
    /// Pretty-print a CSV file
    Show {
        /// File name (from results/)
        file: String,
        /// Max rows to display (0 = unlimited)
        #[arg(long, short = 'n', default_value = "50")]
        limit: usize,
        /// Columns to show (comma-separated)
        #[arg(long, short = 'c', value_delimiter = ',')]
        columns: Option<Vec<String>>,
        /// Filter rows (key=value)
        #[arg(long, short = 'f')]
        filter: Option<String>,
    },
    /// Aggregate summary stats
    Summary,
    /// Side-by-side comparison
    Compare {
        /// First file
        a: String,
        /// Second file
        b: String,
    },
    /// Remove all results (with confirmation)
    Clean,
    /// Open results directory
    Open,
}
