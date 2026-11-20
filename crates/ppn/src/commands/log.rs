#[cfg(test)]
#[path = "../render/log.rs"]
mod render;

use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Show task or board event history.", after_help = example_help(&["ppn log 97ad97", "ppn log 97ad97 --json"]))]
pub struct Args {
    /// Task ID or prefix for task history.
    _input: Option<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn log is not implemented.",
    ))
}
