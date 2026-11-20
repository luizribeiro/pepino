use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Assign people to a task.", after_help = example_help(&["ppn assign 97ad97 me", "ppn assign 97ad97 me --json"]))]
pub struct Args {
    /// Task ID or prefix followed by assignees.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn assign is not implemented.",
    ))
}
