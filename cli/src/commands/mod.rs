mod build;
mod experiment;
mod fault;
mod results;
mod test;

use crate::app::*;
use crate::shell;

pub fn dispatch(cmd: Command) -> anyhow::Result<()> {
    let root = shell::find_project_root().ok_or_else(|| {
        anyhow::anyhow!(
            "Could not find MAFIS project root. \
             Run from inside the project directory."
        )
    })?;

    match cmd {
        Command::Desktop { debug } => build::desktop(&root, debug),
        Command::Experiment { action } => match action {
            ExperimentCommand::List => experiment::list(),
            ExperimentCommand::Run { name } => experiment::run(&root, &name),
            ExperimentCommand::Smoke => experiment::smoke(&root),
            ExperimentCommand::RunAll => experiment::run_all(&root),
        },
        Command::Results { action } => match action {
            ResultsCommand::List => results::list(&root),
            ResultsCommand::Show { file, limit, columns, filter } => {
                results::show(&root, &file, limit, columns.as_deref(), filter.as_deref())
            }
            ResultsCommand::Summary => results::summary(&root),
            ResultsCommand::Compare { a, b } => results::compare(&root, &a, &b),
            ResultsCommand::Clean => results::clean(&root),
            ResultsCommand::Open => results::open(&root),
        },
        Command::Test { filter, release } => test::test(&root, filter.as_deref(), release),
        Command::Serve { no_build, port } => build::serve(&root, no_build, port),
        Command::Build { native } => build::build(&root, native),
        Command::Fault { action } => match action {
            FaultCommand::Run { fault_config } => fault::run_with_config(&root, &fault_config),
            FaultCommand::Validate { fault_config } => fault::validate_config(&fault_config),
        },
    }
}
