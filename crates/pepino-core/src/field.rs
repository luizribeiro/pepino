use serde::{Deserialize, Serialize};

pub use chrono::NaiveDate as Date;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Parent,
    Blocks,
    Relates,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldName(String);

impl FieldName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_custom(&self) -> bool {
        !matches!(
            self.as_str(),
            "title" | "description" | "status" | "due" | "priority"
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    fn distinguishes_builtin_and_custom_fields() {
        for builtin in ["title", "description", "status", "due", "priority"] {
            assert!(!FieldName::new(builtin).is_custom());
        }
        let custom = FieldName::new("estimate");
        assert!(custom.is_custom());
        assert_eq!(custom.as_str(), "estimate");
        assert_eq!(serde_json::to_value(custom).unwrap(), "estimate");
    }
}
