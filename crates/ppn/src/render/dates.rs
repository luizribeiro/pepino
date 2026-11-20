use std::fmt::Write;

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use pepino_app::{Date, Hlc};

use crate::terminal::Terminal;

#[derive(Clone, Copy)]
pub struct Dates<'a> {
    pub today: Date,
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
                .signed_duration_since(NaiveDate::from(self.today))
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

    pub fn timestamp(self, clock: Hlc) -> String {
        let Some(time) = i64::try_from(clock.wall_ms)
            .ok()
            .and_then(DateTime::from_timestamp_millis)
        else {
            return format!("{} ms UTC #{}", clock.wall_ms, clock.counter);
        };
        format!(
            "{} {} UTC #{}",
            format_calendar(time, self.format),
            time.format("%H:%M:%S%.3f"),
            clock.counter
        )
    }
}

fn format_calendar(date: DateTime<Utc>, format: &str) -> String {
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
            today: "2026-11-20".parse().unwrap(),
            format: "%d/%m/%Y",
        };
        let due = "2026-11-21".parse().unwrap();
        assert_eq!(
            dates.due(due, &Terminal::fixed(Some(80), false)),
            "tomorrow"
        );
        assert_eq!(dates.due(due, &Terminal::fixed(None, false)), "21/11/2026");
        assert_eq!(
            dates.timestamp(Hlc {
                wall_ms: 1795149900000,
                counter: 2
            }),
            "20/11/2026 04:45:00.000 UTC #2"
        );
    }
}
