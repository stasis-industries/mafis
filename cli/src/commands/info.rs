use std::path::Path;

use clap::CommandFactory;

use crate::app::Cli;
use crate::logo;
use crate::shell;
use crate::style;

pub fn completions(shell: clap_complete::Shell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "mafis", &mut std::io::stdout());
    Ok(())
}

pub fn version() -> anyhow::Result<()> {
    println!(
        "{} {}",
        style::brand("MAFIS"),
        style::dim(&format!("v{}", logo::VERSION))
    );
    println!(
        "  {}",
        style::dim("Multi-Agent Fault Injection Simulator")
    );

    // Show Rust version too
    if let Ok(rustc) = shell::run_capture(
        "rustc",
        &["--version"],
        &std::env::current_dir().unwrap_or_default(),
    ) {
        println!("  {}", style::dim(&rustc));
    }

    Ok(())
}

pub fn docs(root: &Path, topic: Option<&str>) -> anyhow::Result<()> {
    let docs_dir = root.join("docs");

    match topic {
        Some(name) => {
            // Try to find and open a doc matching the topic
            let candidates = [
                format!("{name}.md"),
                format!("{name}-design.md"),
                format!("{name}-architecture.md"),
            ];

            for candidate in &candidates {
                let path = docs_dir.join(candidate);
                if path.exists() {
                    open::that(&path)?;
                    style::print_success(&format!("Opened docs/{candidate}"));
                    return Ok(());
                }
            }

            // Try CLAUDE.md
            if name == "claude" || name == "readme" || name == "overview" {
                let claude_md = root.join("CLAUDE.md");
                if claude_md.exists() {
                    open::that(&claude_md)?;
                    style::print_success("Opened CLAUDE.md");
                    return Ok(());
                }
            }

            anyhow::bail!("No doc matching '{}'. Run 'docs' to list available.", name);
        }
        None => {
            println!("{}", style::section("Documentation"));
            println!();

            // List CLAUDE.md
            let claude_md = root.join("CLAUDE.md");
            if claude_md.exists() {
                style::kv("CLAUDE.md", "Project instructions & architecture");
            }

            // List docs/
            if docs_dir.exists() {
                for entry in std::fs::read_dir(&docs_dir)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") {
                        let display = name.trim_end_matches(".md");
                        style::kv(&format!("docs/{name}"), display);
                    }
                }
            }

            println!();
            println!("  Open with: {}", style::info("docs <name>"));
        }
    }

    Ok(())
}

pub fn count(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Lines of Code"));
    println!();

    let modules = [
        ("core", "src/core/**/*.rs"),
        ("solver", "src/solver/**/*.rs"),
        ("fault", "src/fault/**/*.rs"),
        ("analysis", "src/analysis/**/*.rs"),
        ("render", "src/render/**/*.rs"),
        ("ui", "src/ui/**/*.rs"),
        ("experiment", "src/experiment/**/*.rs"),
        ("lib+main+constants", "src/*.rs"),
        ("tests", "tests/**/*.rs"),
        ("sim_tests", "src/sim_tests/**/*.rs"),
    ];

    let mut total = 0;

    for (name, pattern) in &modules {
        let full_pattern = root.join(pattern).to_string_lossy().to_string();
        let mut lines = 0;
        let mut files = 0;

        for path in glob::glob(&full_pattern).into_iter().flatten().flatten() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                lines += content.lines().count();
                files += 1;
            }
        }

        if files > 0 {
            style::kv(
                name,
                &format!("{lines:>6} lines ({files} files)"),
            );
            total += lines;
        }
    }

    println!();
    style::kv("TOTAL", &format!("{total:>6} lines"));

    // Web files
    println!();
    let web_patterns = [
        ("JS", "web/*.js"),
        ("HTML", "web/*.html"),
        ("CSS", "web/*.css"),
    ];

    let mut web_total = 0;
    for (name, pattern) in &web_patterns {
        let full_pattern = root.join(pattern).to_string_lossy().to_string();
        let mut lines = 0;

        for path in glob::glob(&full_pattern).into_iter().flatten().flatten() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                lines += content.lines().count();
            }
        }

        if lines > 0 {
            style::kv(name, &format!("{lines:>6} lines"));
            web_total += lines;
        }
    }

    if web_total > 0 {
        println!();
        style::kv("Web total", &format!("{web_total:>6} lines"));
    }

    Ok(())
}

pub fn lint(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Lint"));

    let status = shell::run_streaming(
        "cargo",
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        root,
    )?;

    if !status.success() {
        anyhow::bail!("clippy found warnings/errors");
    }

    style::print_success("No warnings.");
    Ok(())
}
