use std::fmt::Write;

use chrono::{DateTime, NaiveDate, NaiveTime};
use clap::builder::styling::AnsiColor;
use pepino_app::{Date, Hlc, Status, TaskSummary};

use crate::app::InvocationTime;
use crate::terminal::Terminal;

#[derive(Clone, Copy)]
pub struct Dates<'a> {
    pub time: InvocationTime,
    pub format: &'a str,
}

impl Dates<'_> {
    pub fn calendar(self, date: Date) -> String {
        let date = NaiveDate::from(date).and_time(NaiveTime::MIN).and_utc();
        format_calendar(date, self.format)
    }

    pub fn due(self, date: Date, terminal: &Terminal) -> String {
        if terminal.stdout_is_tty {
            match NaiveDate::from(date)
                .signed_duration_since(NaiveDate::from(self.time.today))
                .num_days()
            {
                -1 => return "yesterday".into(),
                0 => return "today".into(),
                1 => return "tomorrow".into(),
                _ => {}
            }
        }
        self.calendar(date)
    }

    pub(super) fn highlight_due(
        self,
        task: &TaskSummary,
        metadata: &mut [String],
        terminal: &Terminal,
        prefix: &str,
    ) {
        if terminal.stdout_is_tty
            && matches!(task.status, Status::Todo | Status::Doing)
            && task.due.is_some_and(|date| date < self.time.today)
            && let Some(label) = metadata.iter_mut().find(|label| label.starts_with(prefix))
        {
            *label = terminal.paint(label, AnsiColor::Red);
        }
    }

    pub fn local_date(self, clock: Hlc) -> String {
        self.local_stamp(clock, self.format)
    }

    pub fn local_date_time(self, clock: Hlc) -> String {
        self.local_stamp(clock, &format!("{} %H:%M", self.format))
    }

    fn local_stamp(self, clock: Hlc, format: &str) -> String {
        let Some(time) = wall_time(clock.wall_ms) else {
            return format!("{} ms since the Unix epoch", clock.wall_ms);
        };
        format_calendar(time.with_timezone(&self.time.offset), format)
    }

    pub fn relative_time(self, clock: Hlc, terminal: &Terminal) -> String {
        if terminal.stdout_is_tty
            && let Some(age) = self.time.now_ms.checked_sub(clock.wall_ms)
        {
            let minutes = age / 60_000;
            let hours = minutes / 60;
            return match (hours, minutes) {
                (0, 0) => "just now".into(),
                (0, 1) => "1 minute ago".into(),
                (0, _) => format!("{minutes} minutes ago"),
                (1, _) => "1 hour ago".into(),
                (2..=23, _) => format!("{hours} hours ago"),
                _ => self.local_date(clock),
            };
        }
        self.local_date(clock)
    }
}

fn wall_time(ms: u64) -> Option<chrono::DateTime<chrono::Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
}

#[cfg(test)]
pub(super) mod tests_support {
    use super::*;

    pub fn invocation_time() -> InvocationTime {
        InvocationTime {
            today: "2026-11-20".parse().unwrap(),
            now_ms: 1_795_157_100_000,
            offset: chrono::FixedOffset::west_opt(18_000).unwrap(),
        }
    }
}

fn format_calendar<T: chrono::TimeZone>(date: DateTime<T>, format: &str) -> String
where
    T::Offset: std::fmt::Display,
{
    let mut output = String::new();
    if write!(output, "{}", date.format(format)).is_ok() {
        output
    } else {
        date.date_naive().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_follow_the_configured_format_and_terminal_context() {
        let dates = Dates {
            time: tests_support::invocation_time(),
            format: "%d/%m/%Y",
        };
        let due = "2026-11-21".parse().unwrap();
        let clock = Hlc {
            wall_ms: 1_795_149_900_000,
            counter: 2,
        };
        assert_eq!(dates.local_date(clock), "19/11/2026");
        assert_eq!(dates.local_date_time(clock), "19/11/2026 23:45");
        assert_eq!(
            dates.relative_time(clock, &Terminal::fixed(None, false)),
            "19/11/2026"
        );
        assert_eq!(
            dates.relative_time(clock, &Terminal::fixed(Some(80), false)),
            "2 hours ago"
        );
        assert_eq!(
            dates.due(due, &Terminal::fixed(Some(80), false)),
            "tomorrow"
        );
        assert_eq!(dates.due(due, &Terminal::fixed(None, false)), "21/11/2026");
    }
}
