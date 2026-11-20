use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{Date, FieldValue, ReadRequest, Request, Status, TaskSummary, WriteRequest};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::input::edit_text;
use ppn::output::CommandResult;
use ppn::render::{merge_write_responses, write_document};
use ppn::response::ResponseExt;
use ppn::terminal::Context;
use ppn::write_args::{AddMembershipArgs, FieldArgs, MembershipArgs};
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

pub fn run(context: &Context<'_>, args: Args, json: bool) -> Result<CommandResult, CliError> {
    let changes = args.fields.changes(context.time.today)?;
    if changes.is_empty() && args.membership.is_empty() {
        return edit_interactively(context, &args.task, json);
    }

    let mut client = AppClient::open(context)?;
    let task = client.task(&args.task)?.id;
    let mut requests = args.membership.requests(&task);
    if !changes.is_empty() {
        requests.insert(0, WriteRequest::SetFields { task, changes });
    }
    apply(context, &mut client, requests)
}

fn apply(
    context: &Context<'_>,
    client: &mut AppClient,
    requests: Vec<WriteRequest>,
) -> Result<CommandResult, CliError> {
    let responses = requests
        .into_iter()
        .map(|request| client.call(Request::Write(request)))
        .collect::<Result<Vec<_>, _>>()?;
    write_document(context, merge_write_responses(&responses)?, "Edited")
}

fn edit_interactively(
    context: &Context<'_>,
    reference: &str,
    json: bool,
) -> Result<CommandResult, CliError> {
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
    let mut after: Form = edit_text(&(text + "\n"), json, |edited| {
        serde_json::from_str(edited)
            .map_err(|error| CliError::new(ErrorKind::Usage, format!("Invalid edit form: {error}")))
    })?;
    after.resolve_assignees(&mut client)?;
    let mut requests = after.membership_from(&before).requests(&task.id);
    requests.insert(
        0,
        WriteRequest::SetFields {
            task: task.id,
            changes: after.changes_from(before)?,
        },
    );
    apply(context, &mut client, requests)
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
    tags: BTreeSet<String>,
    assignees: BTreeSet<String>,
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
            tags: task.tags.clone(),
            assignees: task.assignees.clone(),
        }
    }
}

impl Form {
    fn resolve_assignees(&mut self, client: &mut AppClient) -> Result<(), CliError> {
        if self.assignees.remove("me") {
            let status = client.call(Request::Read(ReadRequest::Status))?;
            self.assignees.insert(status.status()?.identity.id.clone());
        }
        self.assignees = std::mem::take(&mut self.assignees)
            .into_iter()
            .map(|id| id.to_ascii_lowercase())
            .collect();
        Ok(())
    }

    fn membership_from(&self, before: &Self) -> MembershipArgs {
        MembershipArgs {
            add: AddMembershipArgs {
                tags: self.tags.difference(&before.tags).cloned().collect(),
                assignees: self
                    .assignees
                    .difference(&before.assignees)
                    .cloned()
                    .collect(),
            },
            remove_tags: before.tags.difference(&self.tags).cloned().collect(),
            remove_assignees: before
                .assignees
                .difference(&self.assignees)
                .cloned()
                .collect(),
        }
    }

    fn values(self) -> Result<BTreeMap<String, FieldValue>, CliError> {
        let mut fields = self.fields;
        for (name, value) in [
            ("title", FieldValue::Text(self.title)),
            (
                "description",
                self.description.map_or(FieldValue::Unset, FieldValue::Text),
            ),
            ("due", self.due.map_or(FieldValue::Unset, FieldValue::Date)),
            (
                "priority",
                self.priority.map_or(FieldValue::Unset, FieldValue::Integer),
            ),
            ("status", FieldValue::Status(self.status)),
        ] {
            if fields.insert(name.into(), value).is_some() {
                return Err(CliError::new(
                    ErrorKind::Usage,
                    format!("Set {name} at the top level of the edit form."),
                ));
            }
        }
        Ok(fields)
    }

    fn changes_from(self, previous: Self) -> Result<BTreeMap<String, FieldValue>, CliError> {
        let previous = previous.values()?;
        let mut values = self.values()?;
        for key in previous.keys() {
            values.entry(key.clone()).or_insert(FieldValue::Unset);
        }
        values.retain(|key, value| previous.get(key) != Some(value));
        if let Some((name, _)) = values
            .iter()
            .find(|(name, value)| *name != "status" && matches!(value, FieldValue::Status(_)))
        {
            return Err(CliError::new(
                ErrorKind::Usage,
                format!("Use a text, integer, or date value for custom field {name:?}."),
            ));
        }
        Ok(values)
    }
}

fn flags_required() -> CliError {
    CliError::new(ErrorKind::Usage, "Editing without flags requires a terminal.")
        .with_hint("Use --title, --description (or --description - for stdin), --from-file, --due, --priority, --status, --set, --unset, --tag, --untag, --assign, or --unassign.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latch_form() -> Form {
        serde_json::from_value(serde_json::json!({
            "title": "Replace the greenhouse latch",
            "description": "Use stainless fittings.",
            "due": "2026-12-01",
            "priority": 3,
            "status": "doing",
            "fields": {"estimate": {"type": "integer", "value": 2}},
            "tags": ["hardware"],
            "assignees": []
        }))
        .unwrap()
    }

    #[test]
    fn form_differences_clear_fields_and_preserve_unchanged_values() {
        let mut edited = latch_form();
        edited.description = None;
        edited.fields.remove("estimate");
        let changes = edited.changes_from(latch_form()).unwrap();
        assert_eq!(
            changes,
            BTreeMap::from([
                ("description".into(), FieldValue::Unset),
                ("estimate".into(), FieldValue::Unset),
            ])
        );
    }

    #[test]
    fn form_rejects_changes_that_have_no_flag_equivalent() {
        let mut edited = latch_form();
        edited
            .fields
            .insert("inspection".into(), FieldValue::Status(Status::Done));
        let error = edited.changes_from(latch_form()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().contains("inspection"));

        let mut edited = latch_form();
        edited
            .fields
            .insert("priority".into(), FieldValue::Integer(2));
        let error = edited.changes_from(latch_form()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Usage);
        assert!(error.to_string().contains("priority"));
    }
}
