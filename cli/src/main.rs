mod app;
mod commands;
mod logo;
mod repl;
mod shell;
mod style;

use clap::Parser;

fn main() {
    let cli = app::Cli::parse();

    match cli.command {
        Some(cmd) => {
            if let Err(e) = commands::dispatch(cmd) {
                style::print_error(&format!("{e}"));
                std::process::exit(1);
            }
        }
        None => {
            if let Err(e) = repl::run() {
                style::print_error(&format!("{e}"));
                std::process::exit(1);
            }
        }
    }
}
