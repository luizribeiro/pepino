use std::collections::BTreeSet;

use pepino_app::{Request, Response, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{single_line, write_documents};
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Permanently delete tasks. Deletion cannot be undone.", after_help = example_help(&[
    "ppn rm 97ad97 33ea33", "ppn rm 97ad97 --yes --json",
]))]
pub struct Args {
    /// Task IDs or unambiguous prefixes.
    #[arg(required = true, value_name = "TASK")]
    tasks: Vec<String>,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let mut seen = BTreeSet::new();
    let tasks: Vec<_> = client
        .resolve_tasks(args.tasks)?
        .into_iter()
        .filter(|task| seen.insert(task.id.clone()))
        .collect();
    let titles = tasks
        .iter()
        .map(|task| single_line(&task.title))
        .collect::<Vec<_>>();
    let prompt = format!("Permanently delete these tasks?\n  {}", titles.join("\n  "));
    if !context.confirm(&prompt)? {
        let response = Response::Write {
            changed: false,
            created: None,
            targets: vec![],
            tasks: vec![],
            deleted: vec![],
            edges: vec![],
            names: Default::default(),
        };
        return CommandResult::app_document(context, response, |_, _| {
            Ok("Deletion cancelled.".into())
        });
    }

    let responses = tasks
        .into_iter()
        .map(|task| client.call(Request::Write(WriteRequest::Delete { task: task.id })))
        .collect::<Result<Vec<_>, _>>()?;
    write_documents(context, &responses, "Deleted")
}
