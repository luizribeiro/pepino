use pepino_app::{ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::{CommandResult, Warning};
use ppn::render::TaskRows;
use ppn::response::ResponseExt;
use ppn::terminal::Context;
use serde_json::{Map, json};

#[derive(clap::Args)]
#[command(about = "List tasks matching an optional filter.", after_help = example_help(&[
    "ppn ls tag:hardware",
    "ppn ls status:done assignee:me --sort created,-due",
    "ppn ls 'due:<fri' -tag:someday",
    "ppn ls @weekend --limit 20 --json",
    "ppn ls board:Workshop",
    "ppn ls --all-boards --json",
]))]
pub struct Args {
    #[command(flatten)]
    listing: ListingArgs,
    /// Include closed tasks. By default only open tasks are listed unless the filter or a saved view selects a status.
    #[arg(long)]
    all: bool,
    /// List tasks across all boards and include their board in each row.
    #[arg(long)]
    all_boards: bool,
}

#[derive(clap::Args)]
#[group(skip)]
pub(super) struct ListingArgs {
    /// Filter terms used to select tasks. See ppn help filters.
    #[arg(value_name = "FILTER", value_parser = crate::cli::filter::unescape_term)]
    pub(super) filter: Vec<String>,

    /// Parse a complete filter string, ANDed with any positional terms.
    #[arg(
        short = 'f',
        long = "filter",
        value_name = "STRING",
        allow_hyphen_values = true
    )]
    filter_text: Option<String>,

    /// Sort by comma-separated keys (created, updated, due, priority, status, title).
    /// A minus reverses a key.
    #[arg(long, value_name = "SPEC", allow_hyphen_values = true)]
    sort: Option<String>,

    /// Return at most N matching tasks.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    list(context, args.listing, args.all, args.all_boards)
}

pub(super) fn list(
    context: &Context<'_>,
    args: ListingArgs,
    all: bool,
    all_boards: bool,
) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let (filter, positions) = crate::cli::filter::input(args.filter_text, args.filter)?;
    let (sort, limit, today) = (args.sort, args.limit, context.time.today);
    let request = if all {
        ReadRequest::ListTasks {
            filter,
            sort,
            limit,
            today,
        }
    } else {
        ReadRequest::ListOpenTasks {
            filter,
            sort,
            limit,
            today,
        }
    };
    let response = client
        .call_listing(Request::Read(request), all_boards)
        .map_err(|error| positions.remap(error, None))?;

    let result = response.tasks()?;
    if let Some(warning) = result
        .warnings
        .iter()
        .find(|warning| warning.text_tasks.is_none())
    {
        return Err(CliError::new(
            ErrorKind::InvalidInput,
            format!("Unknown filter key {:?}.", warning.field),
        )
        .with_hint(format!("Did you mean {:?}?", warning.suggestion))
        .with_field("filter"));
    }
    let warnings: Vec<_> = result
        .warnings
        .iter()
        .filter_map(|warning| {
            let (count, message) = (warning.text_tasks?, warning.message.as_ref()?);
            Some(Warning {
                message: message.clone(),
                details: Map::from_iter([
                    ("code".into(), json!("numeric_filter_on_text")),
                    ("field".into(), json!(warning.field)),
                    ("text_tasks".into(), json!(count)),
                ]),
            })
        })
        .collect();
    let count = result.tasks.len();
    let note = if count < *result.total {
        Some(format!("showing {count} of {}", result.total))
    } else {
        (count == 0).then(|| "No matching tasks.".into())
    };
    Ok(
        CommandResult::app_document(context, response, |response, settings| {
            let result = response.tasks()?;
            let rows = TaskRows::new(
                result.tasks.iter(),
                result.names,
                settings.dates(),
                &settings.terminal,
            );
            Ok(result
                .tasks
                .iter()
                .map(|task| {
                    let row = rows.row(task, "", "");
                    if let Some(board) = &task.board {
                        format!(
                            "{} ({})  {row}",
                            ppn::render::single_line(&board.name),
                            ppn::render::short_id(&board.id.to_string())
                        )
                    } else {
                        row
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"))
        })?
        .with_human_note(note)
        .with_warnings(warnings),
    )
}
