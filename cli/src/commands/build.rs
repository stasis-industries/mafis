use std::path::Path;
use std::time::Instant;

use owo_colors::OwoColorize;

use crate::shell;
use crate::style;

pub fn check(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Check"));
    let (status, _stdout, stderr) = shell::run_with_spinner(
        "Running cargo check...",
        "cargo",
        &["check"],
        root,
    )?;

    if !status.success() {
        eprintln!("{stderr}");
        anyhow::bail!("cargo check failed");
    }

    Ok(())
}

/// Verify all required tools are installed before attempting a build.
fn preflight_wasm(root: &Path) -> anyhow::Result<()> {
    let mut missing = Vec::new();

    // Check WASM target
    let targets = shell::run_capture("rustup", &["target", "list", "--installed"], root)
        .unwrap_or_default();
    if !targets.contains("wasm32-unknown-unknown") {
        missing.push(
            "wasm32-unknown-unknown target not installed. Fix: rustup target add wasm32-unknown-unknown"
                .to_string(),
        );
    }

    if !shell::has_tool("wasm-bindgen") {
        missing.push(
            "wasm-bindgen not found. Fix: cargo install wasm-bindgen-cli".to_string(),
        );
    }

    if !missing.is_empty() {
        for m in &missing {
            style::print_error(m);
        }
        anyhow::bail!("{} missing dependency(ies)", missing.len());
    }

    Ok(())
}

pub fn build(root: &Path, native: bool) -> anyhow::Result<()> {
    println!("{}", style::section("Build"));

    if native {
        let (status, _stdout, stderr) = shell::run_with_spinner(
            "Building native (release)...",
            "cargo",
            &["build", "--release"],
            root,
        )?;
        if !status.success() {
            eprintln!("{stderr}");
            anyhow::bail!("native build failed");
        }
        style::print_success("Native build complete.");
        return Ok(());
    }

    // Pre-flight: check all deps before compiling
    preflight_wasm(root)?;

    let total_start = Instant::now();

    // Step 1: cargo build for WASM
    let step1 = Instant::now();
    let (status, _stdout, stderr) = shell::run_with_step(
        1,
        2,
        "Compiling WASM (release)...",
        "cargo",
        &["build", "--release", "--target", "wasm32-unknown-unknown"],
        root,
    )?;
    if !status.success() {
        eprintln!("{stderr}");
        anyhow::bail!("WASM build failed");
    }
    let step1_elapsed = step1.elapsed();

    // Step 2: wasm-bindgen
    let step2 = Instant::now();
    let (status, _stdout, stderr) = shell::run_with_step(
        2,
        2,
        "Running wasm-bindgen...",
        "wasm-bindgen",
        &[
            "--out-dir",
            "web",
            "--target",
            "web",
            "target/wasm32-unknown-unknown/release/mafis.wasm",
        ],
        root,
    )?;
    if !status.success() {
        eprintln!("{stderr}");
        anyhow::bail!("wasm-bindgen failed");
    }
    let step2_elapsed = step2.elapsed();

    let total = total_start.elapsed();
    println!();
    style::kv("cargo build", &format!("{:.1}s", step1_elapsed.as_secs_f64()));
    style::kv("wasm-bindgen", &format!("{:.1}s", step2_elapsed.as_secs_f64()));
    style::kv("total", &format!("{:.1}s", total.as_secs_f64()));
    println!();
    style::print_success("WASM build complete.");
    Ok(())
}

pub fn serve(root: &Path, no_build: bool, port: u16) -> anyhow::Result<()> {
    if !no_build {
        build(root, false)?;
        println!();
    }

    println!("{}", style::section("Serve"));

    if !shell::has_tool("basic-http-server") {
        anyhow::bail!(
            "basic-http-server not found. Install with: cargo install basic-http-server"
        );
    }

    let addr = format!("127.0.0.1:{port}");
    let url = format!("http://{addr}");
    println!(
        "  Serving at {}",
        url.truecolor(style::INFO.0, style::INFO.1, style::INFO.2)
    );
    println!("  Press {} to stop.", style::info("Ctrl+C"));
    println!();

    if open::that(&url).is_err() {
        style::print_warning(&format!("Could not open browser. Visit {url} manually."));
    }

    shell::run_streaming("basic-http-server", &["web", "-a", &addr], root)?;

    Ok(())
}

pub fn dev(root: &Path, run_test: bool) -> anyhow::Result<()> {
    use notify::{recommended_watcher, Event, EventKind, RecursiveMode, Watcher};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let mode = if run_test { "test" } else { "check" };

    println!("{}", style::section("Watch Mode"));
    println!(
        "  Watching {} for changes (running {} on save)",
        style::info("src/"),
        style::info(mode),
    );
    println!("  Press {} to stop.", style::info("Ctrl+C"));
    println!();

    let (tx, rx) = mpsc::channel();

    let mut watcher = recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                let has_rs = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|e| e == "rs"));
                if has_rs {
                    let _ = tx.send(());
                }
            }
        }
    })?;

    watcher.watch(&root.join("src"), RecursiveMode::Recursive)?;

    let mut last_check = Instant::now() - Duration::from_secs(10);
    let cmd_args: &[&str] = if run_test { &["test"] } else { &["check"] };

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(()) => {
                if last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    println!(
                        "\n{} Change detected, running cargo {}...",
                        "\u{2500}\u{2500}\u{2500}".truecolor(
                            style::BRAND.0,
                            style::BRAND.1,
                            style::BRAND.2
                        ),
                        mode,
                    );
                    let _ = shell::run_streaming("cargo", cmd_args, root);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

pub fn clean(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Clean"));

    let (status, _, stderr) = shell::run_with_spinner(
        "Running cargo clean...",
        "cargo",
        &["clean"],
        root,
    )?;
    if !status.success() {
        eprintln!("{stderr}");
    }

    let wasm_files = ["web/mafis.js", "web/mafis_bg.wasm"];
    for file in &wasm_files {
        let path = root.join(file);
        if path.exists() {
            std::fs::remove_file(&path)?;
            println!(
                "  {} {}",
                "removed".truecolor(style::DIM.0, style::DIM.1, style::DIM.2),
                file
            );
        }
    }

    style::print_success("Clean complete.");
    Ok(())
}
