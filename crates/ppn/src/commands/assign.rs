use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::write_document;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Assign or unassign people on a task.", after_help = example_help(&["ppn assign 97ad97 me", "ppn assign --remove 97ad97 me --json"]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    #[arg(value_name = "TASK")]
    task: String,
    /// Use me, a display name, or an identity ID or prefix.
    #[arg(required = true, value_name = "WHO")]
    assignees: Vec<String>,
    /// Remove the supplied assignees.
    #[arg(long)]
    remove: bool,
}

pub fn run(
    context: &Context<'_>,
    Args {
        task,
        assignees,
        remove,
    }: Args,
) -> Result<CommandResult, CliError> {
    let (request, verb) = if remove {
        (WriteRequest::Unassign { task, assignees }, "Unassigned")
    } else {
        (WriteRequest::Assign { task, assignees }, "Assigned")
    };
    let response = AppClient::open(context)?.call(Request::Write(request))?;
    write_document(context, response, verb)
}
