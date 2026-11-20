use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Add a comment to a task.", after_help = example_help(&["ppn comment 97ad97 'Use stainless hardware.'", "ppn comment 97ad97 'Use stainless hardware.' --json"]))]
pub struct Args {
    /// Task ID or prefix followed by comment text.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn comment is not implemented.",
    ))
}
