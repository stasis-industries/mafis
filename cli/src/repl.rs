use std::borrow::Cow;
use std::path::{Path, PathBuf};

use clap::Parser;
use owo_colors::OwoColorize;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::FileHistory;
use rustyline::validate::Validator;
use rustyline::{Config, Context, Editor, Helper};

use crate::app::Cli;
use crate::commands;
use crate::logo;
use crate::shell;
use crate::style;

// ---------------------------------------------------------------------------
// Known static values for completions
// ---------------------------------------------------------------------------

const EXPERIMENT_NAMES: &[&str] = &[
    "solver_resilience",
    "scale_sensitivity",
    "scheduler_effect",
    "topology_small",
    "topology_medium",
    "topology_large",
];

// ---------------------------------------------------------------------------
// Context-aware tab completion
// ---------------------------------------------------------------------------

struct MafisHelper {
    commands: Vec<(&'static str, Vec<&'static str>)>,
    result_files: Vec<String>,
}

impl MafisHelper {
    fn new(root: Option<&Path>) -> Self {
        let mut helper = Self {
            commands: vec![
                ("desktop", vec!["--debug"]),
                ("experiment", vec!["list", "run", "smoke", "run-all"]),
                ("results", vec!["list", "show", "summary", "compare", "clean", "open"]),
                ("test", vec!["--release"]),
                ("serve", vec!["--no-build", "--port"]),
                ("build", vec!["--native"]),
                ("help", vec![]),
                ("exit", vec![]),
                ("quit", vec![]),
            ],
            result_files: vec![],
        };

        if let Some(root) = root {
            helper.scan_project(root);
        }

        helper
    }

    fn scan_project(&mut self, root: &Path) {
        // Result files
        if let Ok(entries) = std::fs::read_dir(root.join("results")) {
            self.result_files = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "csv" || ext == "json"))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            self.result_files.sort();
        }
    }

    fn dynamic_completions(&self, cmd: &str, subcmd: &str) -> &[String] {
        match (cmd, subcmd) {
            ("results", "show") | ("results", "compare") => &self.result_files,
            _ => &[],
        }
    }

    fn static_completions(cmd: &str, subcmd: &str) -> &'static [&'static str] {
        match (cmd, subcmd) {
            ("experiment", "run") => EXPERIMENT_NAMES,
            _ => &[],
        }
    }
}

impl Completer for MafisHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let line_to_pos = &line[..pos];
        let parts: Vec<&str> = line_to_pos.split_whitespace().collect();
        let completing_new = line_to_pos.ends_with(' ');

        // Level 1: top-level command
        if parts.is_empty() || (parts.len() == 1 && !completing_new) {
            let prefix = parts.first().copied().unwrap_or("");
            let matches: Vec<Pair> = self
                .commands
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| name.starts_with(prefix))
                .map(|name| Pair { display: name.to_string(), replacement: name.to_string() })
                .collect();
            return Ok((0, matches));
        }

        let cmd = parts[0];

        // Level 2: subcommand (second word)
        if (parts.len() == 1 && completing_new) || (parts.len() == 2 && !completing_new) {
            let prefix = if completing_new { "" } else { parts[1] };
            let word_start = if completing_new { pos } else { pos - prefix.len() };

            if let Some((_, subs)) = self.commands.iter().find(|(name, _)| *name == cmd) {
                let matches: Vec<Pair> = subs
                    .iter()
                    .filter(|s| s.starts_with(prefix))
                    .map(|s| Pair { display: s.to_string(), replacement: s.to_string() })
                    .collect();
                return Ok((word_start, matches));
            }

            return Ok((pos, vec![]));
        }

        // Level 3+: dynamic argument completion
        let subcmd = if parts.len() >= 2 { parts[1] } else { "" };
        let prefix = if completing_new { "" } else { parts.last().copied().unwrap_or("") };
        let word_start = if completing_new { pos } else { pos - prefix.len() };

        // Try dynamic completions first
        let dynamic = self.dynamic_completions(cmd, subcmd);
        if !dynamic.is_empty() {
            return Ok((word_start, complete_from_strings(dynamic, prefix)));
        }

        // Try static completions
        let statics = Self::static_completions(cmd, subcmd);
        if !statics.is_empty() {
            let matches: Vec<Pair> = statics
                .iter()
                .filter(|s| s.starts_with(prefix))
                .map(|s| Pair { display: s.to_string(), replacement: s.to_string() })
                .collect();
            return Ok((word_start, matches));
        }

        Ok((pos, vec![]))
    }
}

fn complete_from_strings(items: &[String], prefix: &str) -> Vec<Pair> {
    items
        .iter()
        .filter(|s| s.starts_with(prefix))
        .map(|s| Pair { display: s.clone(), replacement: s.clone() })
        .collect()
}

impl Hinter for MafisHelper {
    type Hint = String;
}

impl Highlighter for MafisHelper {
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Borrowed(prompt)
    }
}

impl Validator for MafisHelper {}

impl Helper for MafisHelper {}

// ---------------------------------------------------------------------------
// Compact help (for `?` in REPL)
// ---------------------------------------------------------------------------

fn print_compact_help() {
    let (r, g, b) = style::DIM;
    let commands: &[(&str, &str)] = &[
        ("desktop [--debug]", "Launch experiment runner GUI"),
        ("experiment list|run|smoke|run-all", "Run experiments (headless)"),
        ("results list|show|summary|compare|clean|open", "View results"),
        ("test [filter] [--release]", "Run tests"),
        ("serve [--no-build] [--port N]", "Build + serve WASM observatory"),
        ("build [--native]", "WASM or native build"),
        ("exit / quit / Ctrl+D", "Exit"),
    ];
    println!();
    for (cmd, desc) in commands {
        println!(
            "  {:<44} {}",
            cmd.truecolor(style::INFO.0, style::INFO.1, style::INFO.2),
            desc.truecolor(r, g, b),
        );
    }
    println!();
}

// ---------------------------------------------------------------------------
// REPL loop
// ---------------------------------------------------------------------------

fn history_path() -> PathBuf {
    let dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".mafis");
    std::fs::create_dir_all(&dir).ok();
    dir.join("history.txt")
}

pub fn run() -> anyhow::Result<()> {
    logo::print_logo_animated();

    // Find project root (used for prompt + completions)
    let root = shell::find_project_root();

    // Show one-line status
    let branch = root
        .as_ref()
        .and_then(|root| {
            let b = shell::run_capture("git", &["branch", "--show-current"], root).ok()?;
            let wasm_path = root.join("web/mafis_bg.wasm");
            let artifact_status = if wasm_path.exists() {
                if let Ok(meta) = std::fs::metadata(&wasm_path) {
                    use std::time::SystemTime;
                    let age = SystemTime::now()
                        .duration_since(meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))
                        .unwrap_or_default();
                    if age.as_secs() < 3600 {
                        style::success("fresh")
                    } else {
                        style::warning("stale")
                    }
                } else {
                    style::dim("unknown")
                }
            } else {
                style::warning("not built")
            };
            println!(
                "  {} {}  {} {}",
                style::dim("branch:"),
                style::info(&b),
                style::dim("wasm:"),
                artifact_status,
            );
            println!();
            Some(b)
        })
        .unwrap_or_default();

    println!(
        "  Type {} or {} for commands, {} to exit. Tab completes.",
        style::info("help"),
        style::info("?"),
        style::info("Ctrl+D"),
    );
    println!();

    let config = Config::builder().auto_add_history(true).build();
    let mut rl = Editor::<MafisHelper, FileHistory>::with_config(config)?;
    rl.set_helper(Some(MafisHelper::new(root.as_deref())));

    let hist = history_path();
    let _ = rl.load_history(&hist);

    // Build prompt with branch indicator
    let prompt = if branch.is_empty() {
        format!("{} ", "mafis>".truecolor(style::BRAND.0, style::BRAND.1, style::BRAND.2))
    } else {
        format!(
            "{} {} ",
            format!("[{branch}]").truecolor(style::DIM.0, style::DIM.1, style::DIM.2),
            "mafis>".truecolor(style::BRAND.0, style::BRAND.1, style::BRAND.2),
        )
    };

    loop {
        match rl.readline(&prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                match line {
                    "exit" | "quit" => break,
                    "clear" | "cls" => {
                        print!("\x1B[2J\x1B[1;1H");
                    }
                    "?" => print_compact_help(),
                    "help" => {
                        let words: Vec<&str> = vec!["mafis", "--help"];
                        match Cli::try_parse_from(words) {
                            Ok(_) => {}
                            Err(e) => {
                                let _ = e.print();
                            }
                        }
                    }
                    _ => {
                        let words: Vec<&str> =
                            std::iter::once("mafis").chain(line.split_whitespace()).collect();
                        match Cli::try_parse_from(words) {
                            Ok(cli) => {
                                if let Some(cmd) = cli.command {
                                    if let Err(e) = commands::dispatch(cmd) {
                                        style::print_error(&format!("{e}"));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = e.print();
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => {
                style::print_error(&format!("readline: {e}"));
                break;
            }
        }
    }

    let _ = rl.save_history(&hist);
    println!();
    Ok(())
}
