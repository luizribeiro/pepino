use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Manage saved task filters.", after_help = example_help(&["ppn view ls", "ppn view ls --json"]))]
pub struct Args {
    /// Saved-view action and arguments.
    _action: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn view is not implemented.",
    ))
}
