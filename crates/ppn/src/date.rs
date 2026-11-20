use chrono::{Datelike, NaiveDate, TimeDelta};
use pepino_app::Date;

use crate::error::{CliError, ErrorKind};

const WEEKDAYS: &str = "monday tuesday wednesday thursday friday saturday sunday";
const RANGE: &str = "date is out of range";

pub fn resolve_date(value: &str, today: Date) -> Result<Date, CliError> {
    let error = |reason| {
        CliError::new(ErrorKind::InvalidInput, format!("Invalid due date {value:?}: {reason}"))
            .with_field("due")
            .with_hint("Use YYYY-MM-DD, today, tomorrow, yesterday, a lowercase weekday, or a signed offset such as +3d or -2w.")
    };
    parse_date(value, today)
        .map_err(error)?
        .ok_or_else(|| error("unrecognized date"))
}

pub(crate) fn parse_date(value: &str, today: Date) -> Result<Option<Date>, &'static str> {
    if let Ok(date) = value.parse::<Date>()
        && date.to_string() == value
    {
        return Ok(Some(date));
    }
    let today = NaiveDate::from(today);
    let Some(days) = relative_days(value, today) else {
        return Ok(None);
    };
    days.and_then(TimeDelta::try_days)
        .and_then(|delta| today.checked_add_signed(delta))
        .map(|date| Some(date.into()))
        .ok_or(RANGE)
}

fn relative_days(value: &str, today: NaiveDate) -> Option<Option<i64>> {
    let days = match value {
        "today" => 0,
        "tomorrow" => 1,
        "yesterday" => -1,
        _ if let Some(day) = WEEKDAYS
            .split_whitespace()
            .position(|name| value == name || value == &name[..3]) =>
        {
            (day as i64 - i64::from(today.weekday().num_days_from_monday())).rem_euclid(7)
        }
        _ => {
            let (number, multiplier) = value
                .strip_suffix('d')
                .map(|n| (n, 1))
                .or_else(|| value.strip_suffix('w').map(|n| (n, 7)))?;
            return digits(number.strip_prefix(['+', '-'])?)
                .then(|| number.parse::<i64>().ok()?.checked_mul(multiplier));
        }
    };
    Some(Some(days))
}

pub(crate) fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_filters_calendar_forms() {
        let today = "2026-11-20".parse().unwrap();
        for (input, expected) in [
            ("2026-12-01", "2026-12-01"),
            ("today", "2026-11-20"),
            ("tomorrow", "2026-11-21"),
            ("yesterday", "2026-11-19"),
            ("fri", "2026-11-20"),
            ("monday", "2026-11-23"),
            ("-02w", "2026-11-06"),
            ("+3d", "2026-11-23"),
        ] {
            assert_eq!(resolve_date(input, today).unwrap().to_string(), expected);
        }
        for value in ["Friday", "2026-2-01", "+9223372036854775807w", "é"] {
            assert_eq!(
                resolve_date(value, today).unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
        }
    }
}
