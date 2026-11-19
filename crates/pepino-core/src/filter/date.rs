use chrono::{TimeDelta, Weekday};

use super::Date;

pub(super) fn resolve(value: &str, today: Date) -> Option<Date> {
    if let Ok(date) = value.parse::<Date>()
        && date.to_string() == value
    {
        return Some(date);
    }

    let days = match value {
        "today" => 0,
        "tomorrow" => 1,
        "yesterday" => -1,
        _ if value.starts_with(['+', '-']) => offset(value)?,
        _ => {
            use chrono::Datelike;
            let target = weekday(value)?.num_days_from_monday();
            let current = today.weekday().num_days_from_monday();
            i64::from((target + 7 - current) % 7)
        }
    };
    today.checked_add_signed(TimeDelta::try_days(days)?)
}

fn offset(value: &str) -> Option<i64> {
    if let Some(days) = value.strip_suffix('d') {
        return days.parse().ok();
    }

    value.strip_suffix('w')?.parse::<i64>().ok()?.checked_mul(7)
}

fn weekday(value: &str) -> Option<Weekday> {
    match value {
        "mon" | "monday" => Some(Weekday::Mon),
        "tue" | "tuesday" => Some(Weekday::Tue),
        "wed" | "wednesday" => Some(Weekday::Wed),
        "thu" | "thursday" => Some(Weekday::Thu),
        "fri" | "friday" => Some(Weekday::Fri),
        "sat" | "saturday" => Some(Weekday::Sat),
        "sun" | "sunday" => Some(Weekday::Sun),
        _ => None,
    }
}
