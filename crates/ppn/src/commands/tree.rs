#[cfg(test)]
#[path = "../render/tree_guides.rs"]
mod guides;

use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Show the task hierarchy.", after_help = example_help(&["ppn tree 97ad97", "ppn tree 97ad97 --json"]))]
pub struct Args {
    /// Root task ID or prefix.
    _input: Option<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn tree is not implemented.",
    ))
}
