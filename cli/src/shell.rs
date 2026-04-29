use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

use crate::style;

/// Find the MAFIS project root by walking up from CWD.
pub fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let toml = dir.join("Cargo.toml");
        if toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&toml) {
                if content.contains("mafis") {
                    return Some(dir);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Braille spinner style with elapsed time.
fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .tick_strings(&[
            "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{28bf}", "\u{287f}", "\u{289f}", "\u{28af}",
            "\u{28b7}",
        ])
        .template("{spinner:.yellow} {msg}  {elapsed:.dim}")
        .unwrap()
}

/// Run a command with stdout/stderr inherited (streaming output).
pub fn run_streaming(program: &str, args: &[&str], cwd: &Path) -> io::Result<ExitStatus> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

/// Run a command with stdout/stderr inherited and extra environment variables set.
pub fn run_streaming_with_env(
    program: &str,
    args: &[&str],
    cwd: &Path,
    env: &[(String, String)],
) -> io::Result<ExitStatus> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.status()
}

/// Run a command with an animated spinner, capturing output.
pub fn run_with_spinner(
    message: &str,
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> io::Result<(ExitStatus, String, String)> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style());
    spinner.set_message(message.to_string());
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let start = Instant::now();
    let output = Command::new(program).args(args).current_dir(cwd).output()?;

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        spinner.finish_with_message(format!(
            "{} {} ({:.1}s)",
            "\u{2713}".truecolor(style::SUCCESS.0, style::SUCCESS.1, style::SUCCESS.2),
            message,
            elapsed.as_secs_f64()
        ));
    } else {
        spinner.finish_with_message(format!(
            "{} {} (failed in {:.1}s)",
            "\u{2717}".truecolor(style::ERROR.0, style::ERROR.1, style::ERROR.2),
            message,
            elapsed.as_secs_f64()
        ));
    }

    Ok((output.status, stdout, stderr))
}

/// Run a command with a stepped spinner: [step/total] message.
pub fn run_with_step(
    step: usize,
    total: usize,
    message: &str,
    program: &str,
    args: &[&str],
    cwd: &Path,
) -> io::Result<(ExitStatus, String, String)> {
    let label = format!(
        "{} {}",
        format!("[{step}/{total}]").truecolor(style::DIM.0, style::DIM.1, style::DIM.2),
        message,
    );
    run_with_spinner(&label, program, args, cwd)
}

/// Run a command and return its stdout, or an error.
pub fn run_capture(program: &str, args: &[&str], cwd: &Path) -> io::Result<String> {
    let output = Command::new(program).args(args).current_dir(cwd).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check if a program is available on PATH.
pub fn has_tool(name: &str) -> bool {
    which::which(name).is_ok()
}
