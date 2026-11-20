use std::collections::BTreeMap;

use pepino_app::TaskSummary;

use super::row::{metadata_with_due, status_style};
use super::{Dates, ShortIds, single_line};
use crate::terminal::Terminal;

pub struct TaskRows<'a> {
    ids: ShortIds<'a>,
    names: &'a BTreeMap<String, String>,
    dates: Dates<'a>,
    terminal: &'a Terminal,
}

impl<'a> TaskRows<'a> {
    pub fn new(
        tasks: impl Iterator<Item = &'a TaskSummary>,
        names: &'a BTreeMap<String, String>,
        dates: Dates<'a>,
        terminal: &'a Terminal,
    ) -> Self {
        Self {
            ids: ShortIds::new(tasks.map(|task| task.id.as_str())),
            names,
            dates,
            terminal,
        }
    }

    fn metadata(&self, task: &TaskSummary) -> Vec<String> {
        let mut metadata = metadata_with_due(task, self.names, |date| {
            if self.terminal.stdout_is_tty {
                format!("due:{}", self.dates.due(date, self.terminal))
            } else {
                format!("due:{date}")
            }
        });
        if task.blocked {
            metadata.push("blocked".into());
        }
        metadata
    }

    pub fn row(&self, task: &TaskSummary, prefix: &str, suffix: &str) -> String {
        let id = self.ids.get(&task.id);
        let (status, _) = status_style(task.status);
        let title = single_line(&task.title);
        let metadata = self.metadata(task);
        let extra = if metadata.is_empty() { "" } else { "  " };
        let mut text = format!(
            "{prefix}{id} {status}  {title}{extra}{}",
            metadata.join(" ")
        );
        text.push_str(suffix);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn piped_rows_have_stable_status_and_due_date_fields() {
        let mut task = task();
        task.title = "Call the plumber".into();
        task.due = Some("2026-11-20".parse().unwrap());
        task.priority = Some(3);
        task.blocked = false;
        task.tags.clear();
        task.assignees.clear();
        let terminal = Terminal::fixed(None, false);
        let names = BTreeMap::new();
        let rows = TaskRows::new([&task].into_iter(), &names, dates(), &terminal);
        assert_eq!(
            rows.row(&task, "", ""),
            "97ad97 todo  Call the plumber  due:2026-11-20 p3"
        );
    }
}
