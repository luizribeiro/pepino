use std::{fmt, str::FromStr};

use chrono::{Datelike, TimeDelta, Weekday};

use super::{Date, ErrorKind, lex::ascii_digits};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DateOperand {
    Absolute(Date),
    Relative(RelativeDate),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelativeDate(String);

impl DateOperand {
    pub fn resolve(&self, today: Date) -> Result<Date, ErrorKind> {
        match self {
            Self::Absolute(date) => Ok(*date),
            Self::Relative(RelativeDate(value)) => relative_days(value, today)
                .flatten()
                .and_then(TimeDelta::try_days)
                .and_then(|delta| today.checked_add_signed(delta))
                .ok_or_else(|| ErrorKind::DateOutOfRange {
                    value: value.clone(),
                }),
        }
    }
}

impl FromStr for DateOperand {
    type Err = ErrorKind;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(calendar) = calendar(value) {
            return calendar
                .map(Self::Absolute)
                .ok_or_else(|| ErrorKind::DateOutOfRange {
                    value: value.into(),
                });
        }
        if let Some(relative) = relative(value) {
            return Ok(Self::Relative(RelativeDate(relative)));
        }
        let lowercase = value.to_ascii_lowercase();
        if lowercase != value && relative(&lowercase).is_some() {
            return Err(ErrorKind::LowercaseKeyword {
                value: value.into(),
                lowercase,
            });
        }
        Err(ErrorKind::InvalidDate {
            value: value.into(),
        })
    }
}

impl fmt::Display for DateOperand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(date) => date.fmt(f),
            Self::Relative(RelativeDate(value)) => f.write_str(value),
        }
    }
}

pub(super) fn resolve(value: &str, today: Date) -> Result<Date, ErrorKind> {
    value
        .parse::<DateOperand>()?
        .resolve(today)
        .map_err(|_| ErrorKind::DateOutOfRange {
            value: value.into(),
        })
}

fn relative(value: &str) -> Option<String> {
    match value {
        "today" | "tomorrow" | "yesterday" => Some(value.into()),
        _ if weekday(value).is_some() => Some(value[..3].into()),
        _ => {
            let (sign, digits, unit) = offset_parts(value)?;
            let digits = digits.trim_start_matches('0');
            Some(if digits.is_empty() {
                format!("+0{unit}")
            } else {
                format!("{sign}{digits}{unit}")
            })
        }
    }
}

fn calendar(value: &str) -> Option<Option<Date>> {
    let (year_month, day) = value.rsplit_once('-')?;
    let (year, month) = year_month.rsplit_once('-')?;
    let year_digits = year.strip_prefix(['+', '-']).unwrap_or(year);

    // Chrono accepts unpadded components, but filter dates have one calendar spelling.
    if month.len() != 2
        || day.len() != 2
        || year_digits.len() < 4
        || !ascii_digits(year_digits)
        || !ascii_digits(month)
        || !ascii_digits(day)
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
    let (_, _, unit) = offset_parts(value)?;
    let (number, _) = value.split_at(value.len() - 1);
    let multiplier = if unit == "w" { 7 } else { 1 };
    Some(
        number
            .parse::<i64>()
            .ok()
            .and_then(|amount| amount.checked_mul(multiplier)),
    )
}

fn offset_parts(value: &str) -> Option<(&str, &str, &str)> {
    let (number, unit) = value.split_at_checked(value.len().checked_sub(1)?)?;
    let digits = number.strip_prefix(['+', '-'])?;
    (matches!(unit, "d" | "w") && ascii_digits(digits)).then(|| (&number[..1], digits, unit))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_operands_round_trip_in_canonical_form() {
        for (raw, canonical) in [("friday", "fri"), ("-01w", "-1w"), ("-00d", "+0d")] {
            let operand = raw.parse::<DateOperand>().unwrap();
            assert_eq!(operand.to_string(), canonical);
            assert_eq!(canonical.parse(), Ok(operand));
        }
    }
}
