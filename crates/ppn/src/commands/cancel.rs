use pepino_app::Status;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

use super::start::set_status;

#[derive(clap::Args)]
#[command(about = "Cancel tasks.", after_help = example_help(&["ppn cancel 97ad97 33ea33", "ppn cancel 97ad97 --json"]))]
pub struct Args {
    /// Task IDs or unambiguous prefixes.
    #[arg(required = true, value_name = "TASK")]
    tasks: Vec<String>,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    set_status(context, args.tasks, Status::Cancelled, "Cancelled")
}
