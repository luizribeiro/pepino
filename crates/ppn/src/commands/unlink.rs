use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::relation_document;
use ppn::terminal::Context;

use super::link::RelationArgs;

#[derive(clap::Args)]
#[command(about = "Remove a link between two tasks.", after_help = example_help(&[
    "ppn unlink 97ad97 blocks 33ea33", "ppn unlink 97ad97 blocked-by 33ea33 --json",
]))]
pub struct Args {
    #[command(flatten)]
    relation: RelationArgs,
}

pub fn run(context: &Context<'_>, Args { relation }: Args) -> Result<CommandResult, CliError> {
    let (from, kind, to) = relation.endpoints();
    let response = AppClient::open(context)?.call(Request::Write(WriteRequest::Unlink {
        from: from.clone(),
        kind,
        to: to.clone(),
    }))?;
    relation_document(context, response, "Unlinked", (&from, kind, &to))
}
