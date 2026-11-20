use std::collections::BTreeMap;

use clap::builder::styling::AnsiColor;
use pepino_app::TaskSummary;
use unicode_width::UnicodeWidthStr;

use super::row::{metadata_with_due, status_style};
use super::{Dates, ShortIds, single_line, truncate_title};
use crate::terminal::Terminal;

pub struct TaskRows<'a> {
    ids: ShortIds<'a>,
    names: &'a BTreeMap<String, String>,
    dates: Dates<'a>,
    terminal: &'a Terminal,
    widths: [usize; 4],
}

impl<'a> TaskRows<'a> {
    pub fn new(
        tasks: impl Iterator<Item = &'a TaskSummary> + Clone,
        names: &'a BTreeMap<String, String>,
        dates: Dates<'a>,
        terminal: &'a Terminal,
    ) -> Self {
        let mut rows = Self {
            ids: ShortIds::new(tasks.clone().map(|task| task.id.as_str())),
            names,
            dates,
            terminal,
            widths: [0; 4],
        };
        if terminal.stdout_is_tty {
            for task in tasks {
                let widths = [
                    rows.ids.get(&task.id).len(),
                    status_style(task.status).0.len(),
                    single_line(&task.title).width(),
                    rows.metadata(task)
                        .iter()
                        .map(|part| part.width() + 1)
                        .sum::<usize>()
                        .saturating_sub(1),
                ];
                for (width, measured) in rows.widths.iter_mut().zip(widths) {
                    *width = (*width).max(measured);
                }
            }
        }
        rows
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
        let (status, color) = status_style(task.status);
        let title = single_line(&task.title);
        let mut metadata = self.metadata(task);
        let mut text = if self.terminal.stdout_is_tty {
            let [id_width, status_width, title_width, metadata_width] = self.widths;
            let space = self
                .terminal
                .width
                .unwrap_or(usize::MAX)
                .saturating_sub(prefix.width() + suffix.width() + id_width + status_width + 4);
            let reserved = (metadata_width + 2).min(space.saturating_sub(title_width.min(12)));
            let title_width = title_width.min(space.saturating_sub(if metadata.is_empty() {
                0
            } else {
                reserved
            }));
            let title = truncate_title(&title, title_width);
            let mut remaining = space.saturating_sub(title_width + 1);
            let retained = metadata
                .iter()
                .take_while(|part| {
                    remaining.checked_sub(part.width() + 1).is_some_and(|room| {
                        remaining = room;
                        true
                    })
                })
                .count();
            metadata.truncate(retained);
            let id_space = id_width.saturating_sub(id.len()) + 2;
            let status_space = status_width.saturating_sub(status.len()) + 2;
            let id = self.terminal.paint(id, AnsiColor::Cyan);
            let status = self.terminal.paint(status, color);
            let mut line = format!(
                "{prefix}{id}{:id_space$}{status}{:status_space$}{title}",
                "", ""
            );
            if !metadata.is_empty() {
                let title_space = title_width.saturating_sub(title.width()) + 2;
                line.push_str(&" ".repeat(title_space));
            }
            line
        } else {
            let extra = if metadata.is_empty() { "" } else { "  " };
            format!("{prefix}{id} {status}  {title}{extra}")
        };
        self.dates
            .highlight_due(task, &mut metadata, self.terminal, "due:");
        text.push_str(&metadata.join(" "));
        text.push_str(suffix);
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::samples::{dates, task};

    #[test]
    fn listing_columns_align_independently_of_title_width() {
        let mut first = task();
        first.title = "Fit the latch".into();
        let mut second = task();
        second.id = "ab01234f".into();
        second.title = "Repair the garden gate".into();
        second.status = pepino_app::Status::Doing;
        let terminal = Terminal::fixed(Some(100), false);
        let names = BTreeMap::new();
        let rows = TaskRows::new([&first, &second].into_iter(), &names, dates(), &terminal);
        assert_eq!(
            rows.row(&first, "", "").find("#hardware"),
            rows.row(&second, "", "").find("#hardware")
        );
        assert!(
            rows.row(&first, "", "")
                .starts_with("97ad97  todo   Fit the latch")
        );
    }

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
