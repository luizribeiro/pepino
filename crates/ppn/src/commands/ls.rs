use pepino_app::{FilterInput, ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{ShortIds, task_row};
use ppn::response::ResponseExt;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "List tasks matching an optional filter.", after_help = example_help(&[
    "ppn ls tag:hardware",
    "ppn ls status:done assignee:me --sort created,-due",
    "ppn ls 'due:<fri' -- -tag:someday",
    "ppn ls @weekend --limit 20 --json",
]))]
pub struct Args {
    /// Filter terms used to select tasks. Use -- before a term starting with a minus.
    #[arg(value_name = "FILTER")]
    pub(super) filter: Vec<String>,

    /// Sort by comma-separated keys (created, updated, due, priority, status, title).
    /// A minus reverses a key.
    #[arg(long, value_name = "SPEC", allow_hyphen_values = true)]
    sort: Option<String>,

    /// Return at most N matching tasks.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,

    /// Include closed tasks. By default only open tasks are listed unless a status: or is: term is supplied.
    #[arg(long)]
    all: bool,

    /// Select a board by name or full ID.
    #[arg(long, value_name = "BOARD")]
    board: Option<String>,
}

pub fn run(context: &Context<'_>, mut args: Args) -> Result<CommandResult, CliError> {
    if !args.all && !FilterInput::Argv(args.filter.clone()).has_status_term()? {
        args.filter.push("is:open".into());
    }
    if let Some(board) = args.board {
        let board = board.replace('\\', "\\\\").replace('"', "\\\"");
        args.filter.push(format!("board:\"{board}\""));
    }
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::ListTasks {
        filter: FilterInput::Argv(args.filter),
        sort: args.sort,
        limit: args.limit,
        today: context.time.today,
    }))?;

    let result = response.tasks()?;
    let count = result.tasks.len();
    let note = if count < *result.total {
        Some(format!("showing {count} of {}", result.total))
    } else {
        (count == 0).then(|| "No matching tasks.".into())
    };
    Ok(
        CommandResult::app_document(context, response, |response, settings| {
            let result = response.tasks()?;
            let ids = ShortIds::new(result.tasks.iter().map(|task| task.id.as_str()));
            Ok(result
                .tasks
                .iter()
                .map(|task| {
                    task_row(
                        task,
                        &ids,
                        result.names,
                        settings.dates(),
                        &settings.terminal,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        })?
        .with_human_note(note),
    )
}
