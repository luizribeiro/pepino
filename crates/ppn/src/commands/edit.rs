use std::collections::BTreeMap;

use pepino_app::{Date, FieldValue, Request, Status, TaskSummary, WriteRequest};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::input::edit_text;
use ppn::output::CommandResult;
use ppn::render::{merge_write_responses, write_document};
use ppn::terminal::Context;
use ppn::write_args::{FieldArgs, MembershipArgs};
use serde::{Deserialize, Serialize};

#[derive(clap::Args)]
#[command(about = "Change a task’s fields and membership.", long_about = "Change a task’s fields and membership. On a terminal, pass only the task ID to edit a JSON text form in VISUAL or EDITOR.", after_help = example_help(&[
    "ppn edit 97ad97",
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
        return edit_interactively(context, &args.task);
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

fn edit_interactively(context: &Context<'_>, reference: &str) -> Result<CommandResult, CliError> {
    if !(context.terminal.stdin_is_tty && context.terminal.stdout_is_tty) {
        return Err(flags_required());
    }
    let mut client = AppClient::open(context)?;
    let task = client.task(reference)?;
    let before = Form::from(&task);
    let text = serde_json::to_string_pretty(&before).map_err(|error| {
        CliError::new(
            ErrorKind::Failure,
            format!("Could not prepare the edit form: {error}"),
        )
    })?;
    let edited = edit_text(&(text + "\n"))?;
    let after: Form = serde_json::from_str(&edited)
        .map_err(|error| CliError::new(ErrorKind::Usage, format!("Invalid edit form: {error}")))?;
    let response = client.call(Request::Write(WriteRequest::SetFields {
        task: task.id,
        changes: after.changes_from(before),
    }))?;
    write_document(context, response, "Edited")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Form {
    title: String,
    description: Option<String>,
    due: Option<Date>,
    priority: Option<i64>,
    status: Status,
    fields: BTreeMap<String, FieldValue>,
}

impl From<&TaskSummary> for Form {
    fn from(task: &TaskSummary) -> Self {
        Self {
            title: task.title.clone(),
            description: task.description.clone(),
            due: task.due,
            priority: task.priority,
            status: task.status,
            fields: task.fields.clone(),
        }
    }
}

impl Form {
    fn values(self) -> BTreeMap<String, FieldValue> {
        let mut fields = self.fields;
        fields.extend([
            ("title".into(), FieldValue::Text(self.title)),
            (
                "description".into(),
                self.description.map_or(FieldValue::Unset, FieldValue::Text),
            ),
            (
                "due".into(),
                self.due.map_or(FieldValue::Unset, FieldValue::Date),
            ),
            (
                "priority".into(),
                self.priority.map_or(FieldValue::Unset, FieldValue::Integer),
            ),
            ("status".into(), FieldValue::Status(self.status)),
        ]);
        fields
    }

    fn changes_from(self, previous: Self) -> BTreeMap<String, FieldValue> {
        let previous = previous.values();
        let mut values = self.values();
        for key in previous.keys() {
            values.entry(key.clone()).or_insert(FieldValue::Unset);
        }
        values.retain(|key, value| previous.get(key) != Some(value));
        values
    }
}

fn flags_required() -> CliError {
    CliError::new(ErrorKind::Usage, "Editing without flags requires a terminal.")
        .with_hint("Use --title, --description (or --description - for stdin), --from-file, --due, --priority, --status, --set, --unset, --tag, --untag, --assign, or --unassign.")
}
