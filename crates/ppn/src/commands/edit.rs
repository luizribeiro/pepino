use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(hide = true, about = "Change a task’s fields and membership.", after_help = example_help(&["ppn edit 97ad97 --title 'Fix the latch'", "ppn edit 97ad97 --title 'Fix the latch' --json"]))]
pub struct Args {
    /// Task ID or prefix.
    _input: String,
    /// Set the task title.
    #[arg(long)]
    _title: Option<String>,
}

pub fn run(_: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    Err(CliError::new(
        ErrorKind::Failure,
        "ppn edit is not implemented.",
    ))
}
