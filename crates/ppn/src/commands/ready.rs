use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::terminal::Context;

use super::ls;

#[derive(clap::Args)]
#[group(skip)]
#[command(about = "List tasks that are ready to start.", after_help = example_help(&[
    "ppn ready tag:hardware", "ppn ready assignee:me --json",
]))]
pub struct Args {
    #[command(flatten)]
    listing: ls::Args,
}

pub fn run(context: &Context<'_>, mut args: Args) -> Result<CommandResult, CliError> {
    args.listing.filter.push("is:ready".into());
    ls::run(context, args.listing)
}
