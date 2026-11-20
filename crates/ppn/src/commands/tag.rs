use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Add tags to a task.", after_help = example_help(&["ppn tag 97ad97 hardware", "ppn tag 97ad97 hardware --json"]))]
pub struct Args {
    /// Task ID or prefix followed by tags.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn tag is not implemented.",
    ))
}
