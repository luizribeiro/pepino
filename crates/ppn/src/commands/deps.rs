use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Show a task’s blockers and dependents.", after_help = example_help(&["ppn deps 97ad97", "ppn deps 97ad97 --json"]))]
pub struct Args {
    /// Task ID or prefix.
    _input: String,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn deps is not implemented.",
    ))
}
