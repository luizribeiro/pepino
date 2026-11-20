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
    let id = ids.get(&task.id);
    let (marker, color) = match task.status {
        Status::Todo => ("[ ]", AnsiColor::Yellow),
        Status::Doing => ("[~]", AnsiColor::Cyan),
        Status::Done => ("[x]", AnsiColor::Green),
        Status::Cancelled => ("[-]", AnsiColor::BrightBlack),
        _ => ("[?]", AnsiColor::White),
    };
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
    if task.blocked {
        metadata.push("[blocked]".into());
    }
    let suffix = if metadata.is_empty() {
        String::new()
    } else {
        format!("  {}", metadata.join(" "))
    };
    let mut title = single_line(&task.title);
    if let Some(width) = terminal.width.filter(|_| terminal.stdout_is_tty) {
        title = truncate_title(
            &title,
            width.saturating_sub(id.width() + marker.width() + 2 + suffix.width()),
        );
    }
    format!("{id} {} {title}{suffix}", terminal.paint(marker, color))
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
            "97ad97 [ ] Replace the …  #hardware @Ada Gardener due tomorrow p2 [blocked]"
        );
        assert_eq!(
            render(Some(75), true),
            "97ad97 \x1b[33m[ ]\x1b[0m Replace the …  #hardware @Ada Gardener due tomorrow p2 [blocked]"
        );
        assert_eq!(
            render(None, false),
            "97ad97 [ ] Replace the greenhouse latch  #hardware @Ada Gardener due 21/11/2026 p2 [blocked]"
        );
    }
}
