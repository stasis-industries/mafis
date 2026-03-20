use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mafis",
    about = "MAFIS \u{2014} Multi-Agent Fault Injection Simulator",
    version,
    disable_help_subcommand = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    /// Type & borrow check (~5s)
    Check,

    /// Full WASM build pipeline
    Build {
        /// Native-only build (skip WASM)
        #[arg(long)]
        native: bool,
    },

    /// Run tests with streaming output
    Test {
        /// Test name filter
        filter: Option<String>,
        /// Run in release mode
        #[arg(long)]
        release: bool,
    },

    /// Build + serve via HTTP
    Serve {
        /// Skip build, serve existing artifacts
        #[arg(long)]
        no_build: bool,
        /// Port number (1-65535)
        #[arg(long, default_value = "4000", value_parser = clap::value_parser!(u16).range(1..))]
        port: u16,
    },

    /// Watch src/ and auto-check on changes
    Dev {
        /// Run cargo test instead of cargo check
        #[arg(long)]
        test: bool,
    },

    /// Clean build artifacts
    Clean,

    /// Run experiments
    Experiment {
        #[command(subcommand)]
        action: ExperimentCommand,
    },

    /// View and compare results
    Results {
        #[command(subcommand)]
        action: ResultsCommand,
    },

    /// Topology information
    Topology {
        #[command(subcommand)]
        action: TopologyCommand,
    },

    /// Solver information
    Solver {
        #[command(subcommand)]
        action: SolverCommand,
    },

    /// View configuration constants
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Project health check
    Status,

    /// Version info
    Version,

    /// Open documentation
    Docs {
        /// Topic to open
        topic: Option<String>,
    },

    /// Lines of code per module
    Count,

    /// Run clippy lints
    Lint,

    /// Display ASCII art logo
    Logo,

    /// Clear the terminal
    Clear,

    /// Matrix rain animation (press any key to exit)
    Rain,

    /// Random MAPF fact or tip
    Fortune,

    /// Show project structure
    Tree,

    /// Generate shell completions
    Completions {
        /// Shell to generate for (bash, zsh, fish, powershell, elvish)
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum ExperimentCommand {
    /// List all experiment presets
    List,
    /// Run a specific experiment
    Run {
        /// Experiment name (solver_resilience, scale_sensitivity, scheduler_effect, topology_small, topology_medium, topology_large)
        name: String,
    },
    /// Quick smoke test (~1s)
    Smoke,
    /// Run all paper experiments (300 runs)
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

#[derive(Subcommand, Clone, Debug)]
pub enum TopologyCommand {
    /// List available topologies
    List,
    /// Detailed topology info
    Info {
        /// Topology name
        name: String,
    },
    /// ASCII art grid preview
    Preview {
        /// Topology name (small, medium, large, xlarge)
        name: String,
    },
    /// Open map maker in browser
    Mapmaker,
}

#[derive(Subcommand, Clone, Debug)]
pub enum SolverCommand {
    /// List available solvers
    List,
    /// Solver details
    Info {
        /// Solver id
        name: String,
    },
}

#[derive(Subcommand, Clone, Debug)]
pub enum ConfigCommand {
    /// Show all constants
    Show,
    /// Get a specific constant
    Get {
        /// Constant name (e.g. MAX_AGENTS)
        key: String,
    },
}
