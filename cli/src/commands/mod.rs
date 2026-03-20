mod build;
mod config;
mod experiment;
mod fun;
mod info;
mod results;
mod solver;
mod status;
mod test;
mod topology;

use crate::app::*;
use crate::shell;

pub fn dispatch(cmd: Command) -> anyhow::Result<()> {
    // Commands that work without a project root
    if let Command::Completions { shell } = &cmd {
        return info::completions(*shell);
    }

    let root = shell::find_project_root().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not find MAFIS project root. \
             Run from inside the project directory."
        )
    })?;

    match cmd {
        Command::Check => build::check(&root),
        Command::Build { native } => build::build(&root, native),
        Command::Test { filter, release } => test::test(&root, filter.as_deref(), release),
        Command::Serve { no_build, port } => build::serve(&root, no_build, port),
        Command::Dev { test } => build::dev(&root, test),
        Command::Clean => build::clean(&root),
        Command::Experiment { action } => match action {
            ExperimentCommand::List => experiment::list(),
            ExperimentCommand::Run { name } => experiment::run(&root, &name),
            ExperimentCommand::Smoke => experiment::smoke(&root),
            ExperimentCommand::RunAll => experiment::run_all(&root),
        },
        Command::Results { action } => match action {
            ResultsCommand::List => results::list(&root),
            ResultsCommand::Show {
                file,
                limit,
                columns,
                filter,
            } => results::show(
                &root,
                &file,
                limit,
                columns.as_deref(),
                filter.as_deref(),
            ),
            ResultsCommand::Summary => results::summary(&root),
            ResultsCommand::Compare { a, b } => results::compare(&root, &a, &b),
            ResultsCommand::Clean => results::clean(&root),
            ResultsCommand::Open => results::open(&root),
        },
        Command::Topology { action } => match action {
            TopologyCommand::List => topology::list(&root),
            TopologyCommand::Info { name } => topology::info(&root, &name),
            TopologyCommand::Preview { name } => topology::preview(&root, &name),
            TopologyCommand::Mapmaker => topology::mapmaker(&root),
        },
        Command::Solver { action } => match action {
            SolverCommand::List => solver::list(),
            SolverCommand::Info { name } => solver::info(&name),
        },
        Command::Config { action } => match action {
            ConfigCommand::Show => config::show(&root),
            ConfigCommand::Get { key } => config::get(&root, &key),
        },
        Command::Status => status::status(&root),
        Command::Version => info::version(),
        Command::Docs { topic } => info::docs(&root, topic.as_deref()),
        Command::Count => info::count(&root),
        Command::Lint => info::lint(&root),
        Command::Logo => {
            crate::logo::print_logo();
            Ok(())
        }
        Command::Clear => fun::clear(),
        Command::Rain => fun::rain(),
        Command::Fortune => fun::fortune(),
        Command::Tree => fun::tree(&root),
        Command::Completions { .. } => unreachable!(),
    }
}
