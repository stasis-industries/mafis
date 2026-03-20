use std::path::Path;

use crate::shell;
use crate::style;

pub fn status(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Project Status"));
    println!();

    // Git info
    let branch = shell::run_capture("git", &["branch", "--show-current"], root)
        .unwrap_or_else(|_| "unknown".into());
    let dirty = shell::run_capture("git", &["status", "--porcelain"], root)
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    style::kv("Branch", &style::info(&branch));
    let tree_status = if dirty {
        style::warning("dirty")
    } else {
        style::success("clean")
    };
    style::kv("Working tree", &tree_status);

    // Toolchain
    println!();
    let rustc = shell::run_capture("rustc", &["--version"], root).unwrap_or_default();
    style::kv("Rust", &rustc);

    // WASM target
    let targets = shell::run_capture("rustup", &["target", "list", "--installed"], root)
        .unwrap_or_default();
    let has_wasm = targets.contains("wasm32-unknown-unknown");
    let wasm_status = if has_wasm {
        style::success("installed")
    } else {
        style::error("missing (rustup target add wasm32-unknown-unknown)")
    };
    style::kv("WASM target", &wasm_status);

    // Required tools
    println!();
    let tools = [
        ("wasm-bindgen", "cargo install wasm-bindgen-cli"),
        ("basic-http-server", "cargo install basic-http-server"),
    ];

    for (tool, install) in &tools {
        let available = shell::has_tool(tool);
        let status = if available {
            style::success("found")
        } else {
            format!("{} ({})", style::error("missing"), style::dim(install))
        };
        style::kv(tool, &status);
    }

    // Artifact freshness
    println!();
    let wasm_path = root.join("web/mafis_bg.wasm");
    if wasm_path.exists() {
        let meta = std::fs::metadata(&wasm_path)?;
        let size = meta.len();
        let age = std::time::SystemTime::now()
            .duration_since(meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))
            .unwrap_or_default();

        let age_str = if age.as_secs() < 60 {
            format!("{}s ago", age.as_secs())
        } else if age.as_secs() < 3600 {
            format!("{}m ago", age.as_secs() / 60)
        } else if age.as_secs() < 86400 {
            format!("{}h ago", age.as_secs() / 3600)
        } else {
            format!("{}d ago", age.as_secs() / 86400)
        };

        let freshness = if age.as_secs() < 3600 {
            style::success("fresh")
        } else {
            style::warning("stale")
        };

        let artifact_info = format!(
            "{} ({}, {:.1} MB)",
            freshness,
            age_str,
            size as f64 / (1024.0 * 1024.0)
        );
        style::kv("WASM artifact", &artifact_info);
    } else {
        let not_built = style::warning("not built");
        style::kv("WASM artifact", &not_built);
    }

    // Results
    let results_dir = root.join("results");
    if results_dir.exists() {
        let count = std::fs::read_dir(&results_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .count();
        style::kv("Result files", &format!("{count} in results/"));
    } else {
        let none = style::dim("none");
        style::kv("Result files", &none);
    }

    // Source stats
    println!();
    let src_files = glob::glob(&root.join("src/**/*.rs").to_string_lossy())?
        .filter_map(|p| p.ok())
        .count();
    let test_files = glob::glob(&root.join("tests/**/*.rs").to_string_lossy())?
        .filter_map(|p| p.ok())
        .count();
    style::kv("Source files", &format!("{src_files} .rs files in src/"));
    style::kv("Test files", &format!("{test_files} .rs files in tests/"));

    println!();
    Ok(())
}
