use std::path::Path;
use std::time::Instant;

use owo_colors::OwoColorize;

use crate::shell;
use crate::style;

/// Verify all required tools are installed before attempting a build.
fn preflight_wasm(root: &Path) -> anyhow::Result<()> {
    let mut missing = Vec::new();

    let targets =
        shell::run_capture("rustup", &["target", "list", "--installed"], root).unwrap_or_default();
    if !targets.contains("wasm32-unknown-unknown") {
        missing.push(
            "wasm32-unknown-unknown target not installed. Fix: rustup target add wasm32-unknown-unknown"
                .to_string(),
        );
    }

    if !shell::has_tool("wasm-bindgen") {
        missing.push("wasm-bindgen not found. Fix: cargo install wasm-bindgen-cli".to_string());
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

    preflight_wasm(root)?;

    let total_start = Instant::now();

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

pub fn desktop(root: &Path, debug: bool) -> anyhow::Result<()> {
    println!("{}", style::section("Desktop Experiment Runner"));

    if debug {
        println!("  Mode: {} (fast compile, slower runtime)", style::info("debug"));
        let status = shell::run_streaming("cargo", &["run", "--features", "headless"], root)?;
        if !status.success() {
            anyhow::bail!("desktop runner failed");
        }
    } else {
        println!(
            "  Mode: {} (opt-level 3, may take longer to compile)",
            style::info("release-desktop")
        );
        let status = shell::run_streaming(
            "cargo",
            &["run", "--profile", "release-desktop", "--features", "headless"],
            root,
        )?;
        if !status.success() {
            anyhow::bail!("desktop runner failed");
        }
    }

    Ok(())
}

pub fn serve(root: &Path, no_build: bool, port: u16) -> anyhow::Result<()> {
    if !no_build {
        build(root, false)?;
        println!();
    }

    println!("{}", style::section("Serve"));

    if !shell::has_tool("basic-http-server") {
        anyhow::bail!("basic-http-server not found. Install with: cargo install basic-http-server");
    }

    let addr = format!("127.0.0.1:{port}");
    let url = format!("http://{addr}");
    println!("  Serving at {}", url.truecolor(style::INFO.0, style::INFO.1, style::INFO.2));
    println!("  Press {} to stop.", style::info("Ctrl+C"));
    println!();

    if open::that(&url).is_err() {
        style::print_warning(&format!("Could not open browser. Visit {url} manually."));
    }

    shell::run_streaming("basic-http-server", &["web", "-a", &addr], root)?;

    Ok(())
}
