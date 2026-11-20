use pepino_app::{ReadRequest, Request, Response, WriteRequest};
use ppn::{
    app::AppClient,
    error::{CliError, ErrorKind},
    help_tree::example_help,
    output::CommandResult,
    render::{short_id, single_line},
    terminal::Context,
};

#[derive(clap::Args)]
#[command(about = "Show or rename this identity.", after_help = example_help(&["ppn identity show"]))]
pub struct Args {
    #[command(subcommand)]
    operation: Operation,
}

#[derive(clap::Subcommand)]
enum Operation {
    /// Show the identity's display name and device count.
    #[command(after_help = example_help(&["ppn identity show --json"]))]
    Show,
    /// Change the display name across linked devices.
    #[command(after_help = example_help(&["ppn identity rename 'Ada Gardener'"]))]
    Rename {
        /// New display name.
        name: String,
    },
}

impl Args {
    pub(crate) fn writes(&self) -> bool {
        matches!(self.operation, Operation::Rename { .. })
    }
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let request = match args.operation {
        Operation::Show => Request::Read(ReadRequest::IdentityShow),
        Operation::Rename { name } => Request::Write(WriteRequest::IdentityRename { name }),
    };
    let response = AppClient::open(context)?.call_unscoped(request)?;
    CommandResult::app_document(context, response, |response, _| {
        let Response::Identity { identity } = response else {
            return Err(CliError::new(
                ErrorKind::Failure,
                "Expected identity details.",
            ));
        };
        Ok(format!(
            "{} ({}) · {} devices",
            single_line(&identity.display_name),
            short_id(&identity.id),
            identity.roster.len()
        ))
    })
}
