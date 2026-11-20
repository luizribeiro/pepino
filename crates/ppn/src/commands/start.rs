use std::collections::BTreeMap;

use pepino_app::{FieldValue, Request, Status, TaskChange, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::batch_write_document;
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
    let response = AppClient::open(context)?.call(Request::Write(WriteRequest::ApplyToTasks {
        tasks,
        change: TaskChange::SetFields {
            changes: BTreeMap::from([("status".into(), FieldValue::Status(status))]),
        },
    }))?;
    batch_write_document(context, response, verb)
}

#[derive(clap::Args)]
#[command(about = "Reopen tasks as todo.", after_help = example_help(&["ppn reopen 97ad97 33ea33", "ppn reopen 97ad97 --json"]))]
pub struct ReopenArgs {
    /// Task IDs or unambiguous prefixes.
    #[arg(required = true, value_name = "TASK")]
    tasks: Vec<String>,
}

pub fn reopen(context: &Context<'_>, args: ReopenArgs) -> Result<CommandResult, CliError> {
    set_status(context, args.tasks, Status::Todo, "Reopened")
}
