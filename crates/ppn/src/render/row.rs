use std::collections::BTreeMap;

use clap::builder::styling::AnsiColor;
use pepino_app::{Status, TaskSummary};
use unicode_width::UnicodeWidthStr;

use super::{Dates, ShortIds, assignee_label, single_line, truncate_title};
use crate::terminal::Terminal;

pub fn task_row(
    task: &TaskSummary,
    ids: &ShortIds<'_>,
    names: &BTreeMap<String, String>,
    dates: Dates<'_>,
    terminal: &Terminal,
) -> String {
    task_line(
        task,
        ids,
        task_metadata(task, names, dates, terminal),
        terminal,
    )
}

pub(super) fn task_metadata(
    task: &TaskSummary,
    names: &BTreeMap<String, String>,
    dates: Dates<'_>,
    terminal: &Terminal,
) -> Vec<String> {
    let mut metadata = Vec::new();
    metadata.extend(task.tags.iter().map(|tag| format!("#{tag}")));
    metadata.extend(
        task.assignees
            .iter()
            .map(|id| format!("@{}", assignee_label(id, names))),
    );
    metadata.extend(
        task.due
            .map(|date| format!("due {}", dates.due(date, terminal))),
    );
    metadata.extend(task.priority.map(|value| format!("p{value}")));
    metadata
}

pub(super) fn task_line(
    task: &TaskSummary,
    ids: &ShortIds<'_>,
    metadata: Vec<String>,
    terminal: &Terminal,
) -> String {
    let id = ids.get(&task.id);
    let (status, color) = match task.status {
        Status::Todo => ("todo", AnsiColor::Yellow),
        Status::Doing => ("doing", AnsiColor::Cyan),
        Status::Done => ("done", AnsiColor::Green),
        Status::Cancelled => ("cancelled", AnsiColor::BrightBlack),
        _ => ("unknown", AnsiColor::White),
    };
    let status = if task.blocked {
        format!("{status} · blocked")
    } else {
        status.into()
    };
    let mut labels = vec![status.as_str()];
    labels.extend(metadata.iter().map(String::as_str));
    let mut title = single_line(&task.title);
    let mut spacing = 2;
    if let Some(width) = terminal.width.filter(|_| terminal.stdout_is_tty) {
        let room = width.saturating_sub(id.width() + 2).max(1);
        let minimum = title.width().min(12).min(room);
        let mut suffix_width = 0;
        let retained = labels
            .iter()
            .take_while(|label| {
                suffix_width += label.width() + 1;
                minimum + suffix_width < room
            })
            .count();
        labels.truncate(retained);
        let suffix_width = labels.iter().map(|label| label.width() + 1).sum::<usize>();
        let title_width = if labels.is_empty() {
            room
        } else {
            room - suffix_width - 1
        };
        title = truncate_title(&title, title_width);
        spacing += title_width.saturating_sub(title.width());
    }
    let id = terminal.paint(id, AnsiColor::Cyan);
    match labels.split_first() {
        Some((status, rest)) => {
            let status = terminal.paint(status, color);
            let mut output = format!("{id}  {title}{:spacing$}{status}", "");
            for label in rest {
                output.push(' ');
                output.push_str(label);
            }
            output
        }
        None => format!("{id}  {title}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn row_golden_text_at_a_fixed_width() {
        let task = task();
        let names = BTreeMap::from([("a6e1a65f".into(), "Ada Gardener".into())]);
        let ids = ShortIds::new([task.id.as_str()]);
        let render =
            |width, color| task_row(&task, &ids, &names, dates(), &Terminal::fixed(width, color));
        assert_eq!(
            render(Some(75), false),
            "97ad97  Replace the g…  todo · blocked #hardware @Ada Gardener due tomorrow"
        );
        assert_eq!(
            render(Some(75), true),
            "\x1b[36m97ad97\x1b[0m  Replace the g…  \x1b[33mtodo · blocked\x1b[0m #hardware @Ada Gardener due tomorrow"
        );
        assert_eq!(render(Some(20), false), "97ad97  Replace the…");
        assert_eq!(
            render(Some(36), false),
            "97ad97  Replace the…  todo · blocked"
        );
        assert_eq!(
            render(None, false),
            "97ad97  Replace the greenhouse latch  todo · blocked #hardware @Ada Gardener due 21/11/2026 p2"
        );
    }
}
