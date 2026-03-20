use std::path::Path;

use crate::shell;
use crate::style;

pub fn test(root: &Path, filter: Option<&str>, release: bool) -> anyhow::Result<()> {
    println!("{}", style::section("Test"));

    if let Some(f) = filter {
        println!("  Running tests matching: {}", style::info(f));
    }
    if release {
        println!("  Mode: {}", style::info("release"));
    }
    println!();

    let mut args: Vec<&str> = vec!["test"];

    if release {
        args.push("--release");
    }

    if let Some(f) = filter {
        args.push(f);
    }

    args.push("--");
    args.push("--nocapture");

    let status = shell::run_streaming("cargo", &args, root)?;

    if !status.success() {
        anyhow::bail!("tests failed");
    }

    Ok(())
}
