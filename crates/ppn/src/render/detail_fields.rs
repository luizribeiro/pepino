use std::collections::BTreeMap;
use std::fmt::Write;

use pepino_app::{FieldValue, Status, TaskSummary};

use super::{Dates, assignee_label, short_id, single_line};
use crate::terminal::Terminal;

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Todo => "todo",
        Status::Doing => "doing",
        Status::Done => "done",
        Status::Cancelled => "cancelled",
        _ => "unknown",
    }
}

pub(super) fn indented(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{}\n", single_line(line)))
        .collect()
}

pub fn task_fields(
    task: &TaskSummary,
    names: &BTreeMap<String, String>,
    dates: Dates<'_>,
    terminal: &Terminal,
) -> String {
    let list = |values: Vec<String>| {
        if values.is_empty() {
            "-".into()
        } else {
            values.join(", ")
        }
    };
    let tags = list(task.tags.iter().cloned().collect());
    let assignees = list(
        task.assignees
            .iter()
            .map(|id| assignee_label(id, names))
            .collect(),
    );
    let due = task
        .due
        .map_or_else(|| "-".into(), |date| dates.due(date, terminal));
    let priority = task
        .priority
        .map_or_else(|| "-".into(), |priority| priority.to_string());
    let description = task.description.as_deref().map_or_else(
        || ": -\n".into(),
        |text| format!(":\n{}", indented(text, "  ")),
    );
    let yn = |value| if value { "yes" } else { "no" };
    let mut output = format!(
        "Status: {}\nDescription{description}Tags: {tags}\nAssignees: {assignees}\nDue: {due}\nPriority: {priority}\nBlocked: {}\nReady: {}\nAuthor: {}\nCreated: {}\nUpdated: {}\n",
        status_text(task.status),
        yn(task.blocked),
        yn(task.ready),
        short_id(&task.author),
        dates.timestamp(task.created),
        dates.timestamp(task.updated)
    );
    for (key, value) in &task.fields {
        let value = match value {
            FieldValue::Text(value) => single_line(value),
            FieldValue::Integer(value) => value.to_string(),
            FieldValue::Date(value) => dates.calendar(*value),
            FieldValue::Status(value) => status_text(*value).into(),
            FieldValue::Unset => "-".into(),
            _ => "unknown".into(),
        };
        let _ = writeln!(output, "Field {key}: {value}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn empty_optional_fields_have_readable_labels() {
        let mut task = task();
        task.description = None;
        task.tags.clear();
        task.assignees.clear();
        task.due = None;
        task.priority = None;
        task.fields.clear();
        let rendered = task_fields(
            &task,
            &BTreeMap::new(),
            dates(),
            &Terminal::fixed(None, false),
        );
        assert!(rendered.contains("Description: -\nTags: -\nAssignees: -\nDue: -\nPriority: -\n"));
        assert!(!rendered.contains("Field"));
    }
}
