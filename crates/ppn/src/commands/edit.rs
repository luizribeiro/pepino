use pepino_app::{Request, WriteRequest};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{merge_write_responses, write_document};
use ppn::terminal::Context;
use ppn::write_args::{FieldArgs, MembershipArgs};

#[derive(clap::Args)]
#[command(about = "Change a task’s fields and membership.", after_help = example_help(&[
    "ppn edit 97ad97 --title 'Fix the latch'",
    "ppn edit 97ad97 --status doing --tag hardware --json",
]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    #[arg(value_name = "TASK")]
    task: String,
    #[command(flatten)]
    fields: Box<FieldArgs>,
    #[command(flatten)]
    membership: MembershipArgs,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let changes = args.fields.changes(context.time.today)?;
    if changes.is_empty() && args.membership.is_empty() {
        return Err(flags_required());
    }

    let mut client = AppClient::open(context)?;
    let task = client.task(&args.task)?.id;
    let mut requests = args.membership.requests(&task);
    if !changes.is_empty() {
        requests.insert(0, WriteRequest::SetFields { task, changes });
    }
    let responses = requests
        .into_iter()
        .map(|request| client.call(Request::Write(request)))
        .collect::<Result<Vec<_>, _>>()?;
    write_document(context, merge_write_responses(&responses)?, "Edited")
}

fn flags_required() -> CliError {
    CliError::new(ErrorKind::Usage, "Editing without flags requires a terminal.")
        .with_hint("Use --title, --description (or --description - for stdin), --from-file, --due, --priority, --status, --set, --unset, --tag, --untag, --assign, or --unassign.")
}
