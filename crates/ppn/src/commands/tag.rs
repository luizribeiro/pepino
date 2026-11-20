use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::write_document;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Add or remove task tags.", after_help = example_help(&["ppn tag 97ad97 hardware", "ppn tag --remove 97ad97 hardware --json"]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    #[arg(value_name = "TASK")]
    task: String,
    /// Tags to add or remove.
    #[arg(required = true, value_name = "TAG")]
    tags: Vec<String>,
    /// Remove the supplied tags.
    #[arg(long)]
    remove: bool,
}

pub fn run(
    context: &Context<'_>,
    Args { task, tags, remove }: Args,
) -> Result<CommandResult, CliError> {
    let (request, verb) = if remove {
        (WriteRequest::RemoveTags { task, tags }, "Untagged")
    } else {
        (WriteRequest::AddTags { task, tags }, "Tagged")
    };
    let response = AppClient::open(context)?.call(Request::Write(request))?;
    write_document(context, response, verb)
}
