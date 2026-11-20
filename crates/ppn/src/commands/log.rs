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

    /// Show each author's device.
    #[arg(long)]
    verbose: bool,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let authors = client.author_labels(args.verbose);
    let response = client.call(Request::Read(ReadRequest::Log {
        task: args.task,
        limit: (!args.all).then_some(args.limit),
    }))?;
    CommandResult::app_document(context, response, move |response, settings| {
        Ok(render::history(response.log()?, settings.dates(), authors))
    })
}
