use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Mark tasks as done.", after_help = example_help(&["ppn done 97ad97", "ppn done 97ad97 --json"]))]
pub struct Args {
    /// Task IDs or prefixes.
    _input: Vec<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn done is not implemented.",
    ))
}
