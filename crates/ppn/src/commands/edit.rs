use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{Date, FieldValue, ReadRequest, Request, Status, TaskSummary, WriteRequest};

#[path = "edit_relations.rs"]
mod relations;
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::help_tree::example_help;
use ppn::input::{edit_text, form};
use ppn::output::CommandResult;
use ppn::render::{merge_write_responses, write_document};
use ppn::response::ResponseExt;
use ppn::terminal::Context;
use ppn::write_args::{AddMembershipArgs, FieldArgs, MembershipArgs};
#[cfg(test)]
use serde::Deserialize;

#[derive(clap::Args)]
#[command(about = "Change a task’s fields and membership.", long_about = "Change a task’s fields and membership. On a terminal, pass only the task ID to edit a plain text form in VISUAL or EDITOR.", after_help = example_help(&[
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
    #[command(flatten)]
    relations: relations::Args,
}

pub fn run(context: &Context<'_>, args: Args, json: bool) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let changes = args.fields.changes(context.time.today)?;
    if changes.is_empty() && args.membership.is_empty() && args.relations.is_empty() {
        return edit_interactively(context, &mut client, &args.task, json);
    }

    let response = client.call(Request::Read(ReadRequest::GetTask { task: args.task }))?;
    let detail = response.task()?;
    let task = &detail.task.summary;
    let relations = args.relations.changes(task, detail.edges);
    let mut requests = args.membership.requests(&task.id);
    if !changes.is_empty() || (requests.is_empty() && relations.is_empty()) {
        requests.insert(
            0,
            WriteRequest::SetFields {
                task: task.id.clone(),
                changes,
            },
        );
    }
    apply(context, &mut client, requests, relations)
}

fn apply(
    context: &Context<'_>,
    client: &mut AppClient,
    requests: Vec<WriteRequest>,
    relations: Vec<relations::Change>,
) -> Result<CommandResult, CliError> {
    let mut responses = requests
        .into_iter()
        .map(|request| client.call(Request::Write(request)))
        .collect::<Result<Vec<_>, _>>()?;
    for relation in relations {
        responses.push(relation.apply(client)?);
    }
    write_document(context, merge_write_responses(&responses)?, "Edited")
}

fn edit_interactively(
    context: &Context<'_>,
    client: &mut AppClient,
    reference: &str,
    json: bool,
) -> Result<CommandResult, CliError> {
    if !(context.terminal.stdin_is_tty && context.terminal.stdout_is_tty) {
        return Err(flags_required());
    }
    let task = client.task(reference)?;
    let before = Form::from(&task);
    let mut after = edit_text(&before.render(), json, |text| {
        before.parse(text, context.time.today)
    })?;
    after.resolve_assignees(client)?;
    let mut requests = after.membership_from(&before).requests(&task.id);
    requests.insert(
        0,
        WriteRequest::SetFields {
            task: task.id,
            changes: after.changes_from(before)?,
        },
    );
    apply(context, client, requests, vec![])
}

#[cfg_attr(test, derive(Deserialize))]
#[derive(Default)]
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
    fn render(&self) -> String {
        use std::fmt::Write;

        let due = self.due.map(|date| date.to_string()).unwrap_or_default();
        let priority = self
            .priority
            .map(|priority| priority.to_string())
            .unwrap_or_default();
        let status = serde_json::json!(self.status);
        let mut text = format!(
            "# Edit one field per line. Separate tags and assignees with commas.\n# Remove a field.NAME line to clear it. Quote custom text to preserve its type.\ntitle: {}\nstatus: {}\ndue: {due}\npriority: {priority}\ntags: {}\nassignees: {}\n",
            self.title,
            status.as_str().unwrap_or("todo"),
            self.tags.iter().cloned().collect::<Vec<_>>().join(", "),
            self.assignees
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
        for (key, value) in &self.fields {
            let value = match value {
                FieldValue::Text(text) => serde_json::Value::String(text.clone()).to_string(),
                FieldValue::Integer(value) => value.to_string(),
                FieldValue::Date(value) => value.to_string(),
                FieldValue::Status(value) => {
                    format!("existing status {}", serde_json::json!(value))
                }
                _ => continue,
            };
            let _ = writeln!(text, "field.{key}: {value}");
        }
        text.push_str("\n# Description follows. Start a literal # line with \\#.\n");
        text.push_str(&form::escape_description(
            self.description.as_deref().unwrap_or_default(),
        ));
        text
    }

    fn parse(&self, text: &str, today: Date) -> Result<Self, CliError> {
        let (lines, description) = form::parse_form(text)?;
        let has_status = lines.iter().any(|line| line.key == "status");
        let mut after = Self::default();
        for line in lines {
            let value = line.value;
            let invalid = |reason: &str| line.invalid(reason);
            match line.key {
                "title" => after.title = form::title(value).map_err(invalid)?,
                "status" => after.status = form::status(value).map_err(invalid)?,
                "due" if !value.is_empty() => {
                    after.due = Some(
                        ppn::date::resolve_date(value, today)
                            .map_err(|error| line.invalid(error))?,
                    );
                }
                "priority" if !value.is_empty() => {
                    after.priority =
                        Some(value.parse().map_err(|_| invalid("expected an integer"))?);
                }
                "due" | "priority" => {}
                "tags" => after.tags = form::membership(value, true).map_err(invalid)?,
                "assignees" => after.assignees = form::membership(value, false).map_err(invalid)?,
                key if let Some(key) = key.strip_prefix("field.") => {
                    let value = match self.fields.get(key) {
                        Some(FieldValue::Status(status))
                            if value
                                == format!("existing status {}", serde_json::json!(status)) =>
                        {
                            FieldValue::Status(*status)
                        }
                        _ => form::custom_field(key, value, today).map_err(invalid)?,
                    };
                    after.fields.insert(key.into(), value);
                }
                _ => return Err(invalid("unknown field")),
            }
        }
        if after.title.is_empty() || !has_status {
            return Err(CliError::new(
                ErrorKind::Usage,
                "The edit form requires title and status lines.",
            ));
        }
        if description.len() > 65536 {
            return Err(CliError::new(
                ErrorKind::Usage,
                "The edit form description exceeds 65536 bytes.",
            ));
        }
        after.description = (!description.is_empty() || self.description.as_deref() == Some(""))
            .then_some(description);
        Ok(after)
    }

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
