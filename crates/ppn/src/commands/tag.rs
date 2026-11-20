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
    force_remove: bool,
) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    client.check_membership_order(&task, tags.first(), "ppn tag <TASK> <TAG>...")?;
    let (request, verb) = if remove || force_remove {
        (WriteRequest::RemoveTags { task, tags }, "Untagged")
    } else {
        (WriteRequest::AddTags { task, tags }, "Tagged")
    };
    let response = client.call(Request::Write(request))?;
    write_document(context, response, verb)
}
