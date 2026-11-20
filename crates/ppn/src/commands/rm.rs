use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Delete tasks.", after_help = example_help(&["ppn rm 97ad97 --yes", "ppn rm 97ad97 --yes --json"]))]
pub struct Args {
    /// Task IDs or prefixes.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn rm is not implemented.",
    ))
}
