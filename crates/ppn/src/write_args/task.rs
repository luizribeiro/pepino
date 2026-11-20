use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use pepino_app::{Date, FieldValue, Status};

use super::{AddMembershipArgs, CustomArgs};
use crate::date::resolve_date;
use crate::error::{CliError, ErrorKind};
use crate::input::read_task_text;

#[derive(Args, Default)]
pub struct TaskWriteArgs {
    #[command(flatten)]
    pub membership: AddMembershipArgs,
    #[command(flatten)]
    pub relations: RelationArgs,
    #[command(flatten)]
    pub fields: AddFieldArgs,
}

#[derive(Args, Default)]
pub struct AddFieldArgs {
    /// Set the task status.
    #[arg(long, value_enum)]
    pub status: Option<TaskStatus>,
    /// Set the due date. Accepts calendar or relative dates. An empty value or none clears it.
    #[arg(long, allow_hyphen_values = true, value_name = "DATE")]
    pub due: Option<String>,
    /// Set the task priority as a signed integer. An empty value clears it.
    #[arg(long, allow_negative_numbers = true)]
    pub priority: Option<String>,
    #[command(flatten)]
    pub custom: CustomArgs,
    #[command(flatten)]
    pub description: DescriptionArgs,
}

impl AddFieldArgs {
    pub(super) fn changes(self, today: Date) -> Result<BTreeMap<String, FieldValue>, CliError> {
        let mut fields = BTreeMap::new();
        if let Some(due) = self.due {
            let value = if due.is_empty() || due.eq_ignore_ascii_case("none") {
                FieldValue::Unset
            } else {
                FieldValue::Date(resolve_date(&due, today)?)
            };
            fields.insert("due".into(), value);
        }
        if let Some(description) = self.description.read()? {
            let value = if description.is_empty() {
                FieldValue::Unset
            } else {
                FieldValue::Text(description)
            };
            fields.insert("description".into(), value);
        }
        if let Some(priority) = self.priority {
            let value = if priority.is_empty() {
                FieldValue::Unset
            } else {
                FieldValue::Integer(priority.parse().map_err(|_| {
                    CliError::new(
                        ErrorKind::Usage,
                        format!(
                            "Invalid --priority {priority:?}: expected a signed 64-bit integer"
                        ),
                    )
                })?)
            };
            fields.insert("priority".into(), value);
        }
        if let Some(status) = self.status {
            fields.insert("status".into(), FieldValue::Status(status.into()));
        }
        self.custom.apply(today, &mut fields)?;
        Ok(fields)
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
    Cancelled,
}

impl From<TaskStatus> for Status {
    fn from(value: TaskStatus) -> Self {
        match value {
            TaskStatus::Todo => Self::Todo,
            TaskStatus::Doing => Self::Doing,
            TaskStatus::Done => Self::Done,
            TaskStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Args, Default)]
pub struct RelationArgs {
    /// Set the parent task ID or prefix.
    #[arg(long, value_name = "TASK")]
    pub parent: Option<String>,
    /// Block a task by ID or prefix. Repeat for more dependencies.
    #[arg(long, value_name = "TASK")]
    pub blocks: Vec<String>,
    /// Depend on a task by ID or prefix. Repeat for more dependencies.
    #[arg(long, value_name = "TASK")]
    pub blocked_by: Vec<String>,
}

impl RelationArgs {
    pub(super) fn apply(self, request: &mut pepino_app::WriteRequest) {
        if let pepino_app::WriteRequest::AddTask {
            parent,
            blocks,
            blocked_by,
            ..
        } = request
        {
            *parent = self.parent.or_else(|| parent.take());
            blocks.extend(self.blocks);
            blocked_by.extend(self.blocked_by);
        }
    }
}

#[derive(Args, Default)]
pub struct DescriptionArgs {
    /// Set the description. Use - for stdin or an empty value to clear it.
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    /// Read the description from a file.
    #[arg(long, value_name = "PATH", conflicts_with = "description")]
    from_file: Option<PathBuf>,
}

impl DescriptionArgs {
    pub fn reads_stdin(&self) -> bool {
        self.description.as_deref() == Some("-")
    }

    pub fn read(self) -> Result<Option<String>, CliError> {
        if self.description.is_none() && self.from_file.is_none() {
            return Ok(None);
        }
        read_task_text(self.description, self.from_file.as_deref()).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Edit {
        #[command(flatten)]
        args: TaskWriteArgs,
    }

    #[test]
    fn parses_shared_flags_and_distinguishes_text_sources() {
        let edit = Edit::try_parse_from([
            "edit",
            "-t",
            "release",
            "--tag",
            "docs",
            "--due",
            "-1w",
            "--priority",
            "-2",
            "--assign",
            "me",
            "--description",
            "-",
        ])
        .unwrap()
        .args;
        assert_eq!(edit.membership.tags, ["release", "docs"]);
        assert_eq!(edit.fields.due.as_deref(), Some("-1w"));
        assert_eq!(edit.fields.priority.as_deref(), Some("-2"));
        assert_eq!(edit.membership.assignees, ["me"]);
        assert!(edit.fields.description.reads_stdin());
        let edit = Edit::try_parse_from(["edit", "--description", "Check release notes"]).unwrap();
        assert_eq!(
            edit.args.fields.description.read().unwrap().as_deref(),
            Some("Check release notes")
        );
        assert_eq!(DescriptionArgs::default().read().unwrap(), None);
        assert!(
            Edit::try_parse_from(["edit", "--description", "-", "--from-file", "notes.txt"])
                .is_err()
        );
    }
}
