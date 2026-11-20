use pepino_app::{ReadRequest, Request, Response, TaskChange, WriteRequest};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{batch_write_document, describe_delete_impact, single_line};
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
    let Response::DeletePreview {
        targets,
        tasks,
        top_level,
        unblocked,
    } = client.call(Request::Read(ReadRequest::PreviewDelete {
        tasks: args.tasks,
    }))?
    else {
        return Err(CliError::new(
            ErrorKind::Failure,
            "Expected a deletion preview.",
        ));
    };
    let impact = describe_delete_impact(&top_level, &unblocked, false);
    let titles = tasks
        .iter()
        .map(|task| single_line(&task.title))
        .collect::<Vec<_>>();
    let titles = titles.join("\n  ");
    let prompt = format!("Permanently delete these tasks?\n  {titles}{impact}");
    let count = tasks.len();
    let noun = if count == 1 { "task" } else { "tasks" };
    let refusal =
        format!("Refusing to delete {count} {noun} without confirmation\n  {titles}{impact}");
    if !tasks.is_empty() && !context.confirm_with_refusal(&prompt, &refusal)? {
        let response = Response::Write {
            changed: false,
            created: None,
            targets: vec![],
            tasks: vec![],
            deleted: vec![],
            activated: vec![],
            top_level: vec![],
            unblocked: vec![],
            edges: vec![],
            names: Default::default(),
        };
        return CommandResult::app_document(context, response, |_, _| {
            Ok("Deletion cancelled.".into())
        });
    }

    let response = client.call(Request::Write(WriteRequest::ApplyToTasks {
        tasks: targets,
        change: TaskChange::Delete,
    }))?;
    batch_write_document(context, response, "Deleted")
}
