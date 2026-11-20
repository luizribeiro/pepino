use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "List tasks matching an optional filter.", after_help = example_help(&["ppn ls tag:hardware", "ppn ls tag:hardware --json"]))]
pub struct Args {
    /// Filter terms used to select tasks.
    _filter: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn ls is not implemented.",
    ))
}
