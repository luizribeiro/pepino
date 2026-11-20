use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Remove a link between two tasks.", after_help = example_help(&["ppn unlink 97ad97 blocks 33ea33", "ppn unlink 97ad97 blocks 33ea33 --json"]))]
pub struct Args {
    /// Source task, relation, and destination task.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn unlink is not implemented.",
    ))
}
