use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;

pub use chrono::NaiveDate as Date;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Todo,
    Doing,
    Done,
    Cancelled,
}

impl Status {
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Todo | Self::Doing)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Parent,
    Blocks,
    Relates,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FieldName(String);

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidFieldName {
    #[error(
        "field names must start with a lowercase ASCII letter, use only lowercase ASCII letters, digits, underscores or hyphens, and be at most 64 bytes"
    )]
    Syntax,
    #[error("field name is reserved")]
    Reserved,
}

impl FieldName {
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidFieldName> {
        let field = Self::from_event_name(name.into());
        field.validate()?;
        Ok(field)
    }

    pub(crate) fn from_event_name(name: String) -> Self {
        Self(name)
    }

    pub(crate) fn validate(&self) -> Result<(), InvalidFieldName> {
        if self.0.len() > 64 || !valid_name(&self.0, b"") {
            return Err(InvalidFieldName::Syntax);
        }
        if matches!(self.as_str(), "tags" | "assignees" | "id") {
            return Err(InvalidFieldName::Reserved);
        }
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_custom(&self) -> bool {
        !matches!(
            self.as_str(),
            "title" | "description" | "status" | "due" | "priority" | "tags" | "assignees" | "id"
        )
    }
}

impl<'de> Deserialize<'de> for FieldName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

pub(crate) fn valid_name(name: &str, extra: &[u8]) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-') || extra.contains(&c))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum FieldValue {
    Text(String),
    Integer(i64),
    Date(Date),
    Status(Status),
    Unset,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_edge_kinds_have_fixed_names() {
        for (status, name, open) in [
            (Status::Todo, "todo", true),
            (Status::Doing, "doing", true),
            (Status::Done, "done", false),
            (Status::Cancelled, "cancelled", false),
        ] {
            assert_eq!(status.is_open(), open);
            assert_eq!(serde_json::to_value(status).unwrap(), name);
        }
        for (kind, name) in [
            (EdgeKind::Parent, "parent"),
            (EdgeKind::Blocks, "blocks"),
            (EdgeKind::Relates, "relates"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    #[test]
    fn rejects_illegal_and_reserved_field_names() {
        for bad in [
            "",
            "9patch",
            "Title",
            "due/date",
            "garden.tools",
            "café",
            "garden tools",
        ] {
            assert_eq!(FieldName::new(bad), Err(InvalidFieldName::Syntax));
            assert!(serde_json::from_value::<FieldName>(bad.into()).is_err());
        }
        for reserved in ["tags", "assignees", "id"] {
            assert_eq!(FieldName::new(reserved), Err(InvalidFieldName::Reserved));
        }
        assert!(FieldName::new("a".repeat(64)).is_ok());
        assert_eq!(
            FieldName::new("a".repeat(65)),
            Err(InvalidFieldName::Syntax)
        );
        assert!(FieldName::new("review_2-done").is_ok());
    }

    #[test]
    fn distinguishes_builtin_and_custom_fields() {
        for builtin in ["title", "description", "status", "due", "priority"] {
            assert!(!FieldName::new(builtin).unwrap().is_custom());
        }
        let custom = FieldName::new("estimate").unwrap();
        assert!(custom.is_custom());
        assert_eq!(custom.as_str(), "estimate");
        assert_eq!(serde_json::to_value(custom).unwrap(), "estimate");
    }
}
