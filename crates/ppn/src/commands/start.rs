use std::collections::BTreeMap;

use pepino_app::{FieldValue, Request, Status, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::write_documents;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Mark tasks as in progress.", after_help = example_help(&["ppn start 97ad97 33ea33", "ppn start 97ad97 --json"]))]
pub struct Args {
    /// Task IDs or unambiguous prefixes.
    #[arg(required = true, value_name = "TASK")]
    tasks: Vec<String>,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    set_status(context, args.tasks, Status::Doing, "Started")
}

pub(super) fn set_status(
    context: &Context<'_>,
    tasks: Vec<String>,
    status: Status,
    verb: &str,
) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let tasks = client.resolve_tasks(tasks)?;
    let responses = tasks
        .into_iter()
        .map(|task| {
            client.call(Request::Write(WriteRequest::SetFields {
                task: task.id,
                changes: BTreeMap::from([("status".into(), FieldValue::Status(status))]),
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    write_documents(context, &responses, verb)
}
