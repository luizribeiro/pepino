use std::path::PathBuf;

use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::input::read_task_text;
use ppn::output::CommandResult;
use ppn::render::write_document;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Add a comment to a task.", after_help = example_help(&[
    "ppn comment 97ad97 'Use stainless hardware.'",
    "ppn comment 97ad97 --from-file notes.txt --json",
]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    #[arg(value_name = "TASK")]
    task: String,
    /// Comment text. Use - to read it from stdin.
    text: Option<String>,
    /// Read the comment from a file.
    #[arg(long, value_name = "PATH", conflicts_with = "text")]
    from_file: Option<PathBuf>,
}

pub fn run(
    context: &Context<'_>,
    Args {
        task,
        text,
        from_file,
    }: Args,
) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let text = read_task_text(text, from_file.as_deref())?;
    let response = client.call(Request::Write(WriteRequest::Comment { task, text }))?;
    write_document(context, response, "Commented on")
}
