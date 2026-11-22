use pepino_app::{ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::response::ResponseExt;
use ppn::terminal::Context;

#[path = "../render/log.rs"]
mod render;

#[derive(clap::Args)]
#[command(about = "Show task or board event history, oldest first.", after_help = example_help(&[
    "ppn log 97ad97 --all", "ppn log --limit 10 --json",
]))]
pub struct Args {
    /// Task ID or prefix. Omit it for the whole board.
    #[arg(value_name = "TASK")]
    task: Option<String>,

    /// Show the newest N ordered events.
    #[arg(long, value_name = "N", default_value_t = 50)]
    limit: usize,

    /// Show the full history regardless of --limit.
    #[arg(long)]
    all: bool,
}

pub fn run(context: &Context<'_>, args: Args, verbose: bool) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let authors = client.author_labels(verbose);
    let history = if args.task.is_none() {
        super::board::history::records(client.local_state(), client.selected_board()?.0.id)
    } else {
        Vec::new()
    };
    let history = if !args.all && history.len() > args.limit {
        history[history.len() - args.limit..].to_vec()
    } else {
        history
    };
    let human_history = history.clone();
    let response = client.call(Request::Read(ReadRequest::Log {
        task: args.task,
        limit: (!args.all).then_some(args.limit),
    }))?;
    CommandResult::app_document_with_json(
        context,
        response,
        move |response, settings| {
            let tasks = render::history(response.log()?, settings.dates(), authors);
            if human_history.is_empty() {
                return Ok(tasks);
            }
            let membership = human_history
                .iter()
                .map(|row| super::board::history::line(row, settings.dates()))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!("{tasks}\n{membership}"))
        },
        move |fields| {
            if !history.is_empty() {
                fields.insert("membership_history".into(), serde_json::json!(history));
            }
        },
    )
}
