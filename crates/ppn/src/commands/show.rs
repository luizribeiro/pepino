use pepino_app::{ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::task_detail;
use ppn::response::{ResponseExt, Task};
use ppn::terminal::Context;

#[derive(clap::Args)]
/// Show a task, its relationships, and comments.
#[command(after_help = example_help(&["ppn show 97ad97", "ppn show 97ad97 --json"]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    task: String,
    /// Show each author's device.
    #[arg(long)]
    verbose: bool,
}

pub fn run(context: &Context<'_>, Args { task, verbose }: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let authors = client.author_labels(verbose);
    let response = client.call(Request::Read(ReadRequest::GetTask { task }))?;
    CommandResult::app_document(context, response, move |response, settings| {
        let Task {
            task,
            edges,
            relation_titles,
            names,
            ..
        } = response.task()?;
        Ok(task_detail(
            task,
            edges,
            relation_titles,
            names,
            settings.dates(),
            &settings.terminal,
            &authors,
        ))
    })
}
