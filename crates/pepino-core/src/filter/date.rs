use chrono::{Datelike, TimeDelta, Weekday};

use super::{Date, ErrorKind};

pub(super) fn resolve(value: &str, today: Date) -> Result<Date, ErrorKind> {
    let invalid = || ErrorKind::InvalidDate {
        value: value.into(),
    };
    let overflow = || ErrorKind::DateOutOfRange {
        value: value.into(),
    };
    if let Some(calendar) = calendar(value) {
        return calendar.ok_or_else(overflow);
    }
    let Some(days) = relative_days(value, today) else {
        let lowercase = value.to_ascii_lowercase();
        if lowercase != value && relative_days(&lowercase, today).is_some() {
            return Err(ErrorKind::LowercaseKeyword {
                value: value.into(),
                lowercase,
            });
        }
        return Err(invalid());
    };
    days.and_then(TimeDelta::try_days)
        .and_then(|delta| today.checked_add_signed(delta))
        .ok_or_else(overflow)
}

fn calendar(value: &str) -> Option<Option<Date>> {
    let (year_month, day) = value.rsplit_once('-')?;
    let (year, month) = year_month.rsplit_once('-')?;
    let year_digits = year.strip_prefix(['+', '-']).unwrap_or(year);

    // Chrono accepts unpadded components, but filter dates have one calendar spelling.
    if month.len() != 2
        || day.len() != 2
        || year_digits.len() < 4
        || !digits(year_digits)
        || !digits(month)
        || !digits(day)
    {
        return None;
    }
    let month = month.parse().ok().filter(|n| (1..=12).contains(n))?;
    let day = day.parse().ok().filter(|n| (1..=31).contains(n))?;
    if year_digits.len() > 4 && !year.starts_with(['+', '-']) {
        return None;
    }
    let Ok(year) = year.parse::<i32>() else {
        return Some(None);
    };
    if !(Date::MIN.year()..=Date::MAX.year()).contains(&year) {
        return Some(None);
    }
    let date = Date::from_ymd_opt(year, month, day)?;
    (date.to_string() == value).then_some(Some(date))
}

fn relative_days(value: &str, today: Date) -> Option<Option<i64>> {
    let days = match value {
        "today" => 0,
        "tomorrow" => 1,
        "yesterday" => -1,
        _ if value.starts_with(['+', '-']) => return offset(value),
        _ => {
            let target = weekday(value)?.num_days_from_monday();
            let current = today.weekday().num_days_from_monday();
            i64::from((target + 7 - current) % 7)
        }
    };
    Some(Some(days))
}

fn offset(value: &str) -> Option<Option<i64>> {
    let (number, unit) = value.split_at_checked(value.len().checked_sub(1)?)?;
    let multiplier = match unit {
        "d" => 1,
        "w" => 7,
        _ => return None,
    };
    let magnitude = number.strip_prefix(['+', '-'])?;
    digits(magnitude).then(|| number.parse::<i64>().ok()?.checked_mul(multiplier))
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
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
