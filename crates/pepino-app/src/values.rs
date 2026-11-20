use std::{fmt, str::FromStr};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::state::storage;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
    #[default]
    Todo,
    Doing,
    Done,
    Cancelled,
}

impl From<Status> for pepino_core::Status {
    fn from(value: Status) -> Self {
        match value {
            Status::Todo => Self::Todo,
            Status::Doing => Self::Doing,
            Status::Done => Self::Done,
            Status::Cancelled => Self::Cancelled,
        }
    }
}

impl TryFrom<pepino_core::Status> for Status {
    type Error = AppError;

    fn try_from(value: pepino_core::Status) -> Result<Self, Self::Error> {
        use pepino_core::Status as Core;
        Ok(match value {
            Core::Todo => Self::Todo,
            Core::Doing => Self::Doing,
            Core::Done => Self::Done,
            Core::Cancelled => Self::Cancelled,
            _ => return Err(unsupported_value("status")),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EdgeKind {
    Parent,
    Blocks,
    Relates,
}

impl From<EdgeKind> for pepino_core::EdgeKind {
    fn from(value: EdgeKind) -> Self {
        match value {
            EdgeKind::Parent => Self::Parent,
            EdgeKind::Blocks => Self::Blocks,
            EdgeKind::Relates => Self::Relates,
        }
    }
}

impl TryFrom<pepino_core::EdgeKind> for EdgeKind {
    type Error = AppError;

    fn try_from(value: pepino_core::EdgeKind) -> Result<Self, Self::Error> {
        use pepino_core::EdgeKind as Core;
        Ok(match value {
            Core::Parent => Self::Parent,
            Core::Blocks => Self::Blocks,
            Core::Relates => Self::Relates,
        })
    }
}

fn unsupported_value(kind: &str) -> AppError {
    storage(
        "project stored value",
        format_args!("unsupported {kind}"),
        false,
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Date(NaiveDate);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("expected a calendar date in YYYY-MM-DD format")]
pub struct InvalidDate;

impl FromStr for Date {
    type Err = InvalidDate;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        NaiveDate::parse_from_str(input, "%Y-%m-%d")
            .map(Self)
            .map_err(|_| InvalidDate)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.format("%Y-%m-%d"), formatter)
    }
}

impl TryFrom<String> for Date {
    type Error = InvalidDate;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Date> for String {
    fn from(value: Date) -> Self {
        value.to_string()
    }
}

impl From<NaiveDate> for Date {
    fn from(value: NaiveDate) -> Self {
        Self(value)
    }
}

impl From<Date> for NaiveDate {
    fn from(value: Date) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum FieldValue {
    Text(String),
    Integer(i64),
    Date(Date),
    Status(Status),
    Unset,
}

impl From<FieldValue> for pepino_core::FieldValue {
    fn from(value: FieldValue) -> Self {
        match value {
            FieldValue::Text(value) => Self::Text(value),
            FieldValue::Integer(value) => Self::Integer(value),
            FieldValue::Date(value) => Self::Date(value.into()),
            FieldValue::Status(value) => Self::Status(value.into()),
            FieldValue::Unset => Self::Unset,
        }
    }
}

impl TryFrom<pepino_core::FieldValue> for FieldValue {
    type Error = AppError;

    fn try_from(value: pepino_core::FieldValue) -> Result<Self, Self::Error> {
        use pepino_core::FieldValue as Core;
        Ok(match value {
            Core::Text(value) => Self::Text(value),
            Core::Integer(value) => Self::Integer(value),
            Core::Date(value) => Self::Date(value.into()),
            Core::Status(value) => Self::Status(value.try_into()?),
            Core::Unset => Self::Unset,
            _ => return Err(unsupported_value("field value")),
        })
    }
}
