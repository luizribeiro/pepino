use std::collections::BTreeMap;
use std::fmt::Write;

use pepino_app::{FieldValue, Status, TaskSummary};

use super::row::task_metadata;
use super::{Dates, assignee_label, single_line};
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
    let mut output = String::new();
    if let Some(description) = task.description.as_deref().filter(|text| !text.is_empty()) {
        output.push_str(&indented(description, ""));
        output.push('\n');
    }
    let metadata = task_metadata(task, names, dates, terminal);
    if !metadata.is_empty() {
        let _ = writeln!(output, "{}", metadata.join("  "));
    }
    for (key, value) in &task.fields {
        let value = match value {
            FieldValue::Text(value) => single_line(value),
            FieldValue::Integer(value) => value.to_string(),
            FieldValue::Date(value) => dates.calendar(*value),
            FieldValue::Status(value) => status_text(*value).into(),
            FieldValue::Unset => continue,
            _ => "unknown".into(),
        };
        let _ = writeln!(output, "{key}: {value}");
    }
    output
}

pub(super) fn task_timestamps(
    task: &TaskSummary,
    names: &BTreeMap<String, String>,
    dates: Dates<'_>,
    terminal: &Terminal,
) -> String {
    format!(
        "created {} by {} · updated {}\n",
        dates.local_date(task.created),
        assignee_label(&task.author, names),
        dates.relative_time(task.updated, terminal)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn empty_optional_fields_are_omitted() {
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
        assert!(rendered.is_empty());
    }
}
