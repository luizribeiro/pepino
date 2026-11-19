use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Date, EdgeKind, FieldName, FieldValue, IdentityId, InvalidFieldName, Status, TaskId,
    field::valid_name,
};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitialFields {
    pub description: Option<String>,
    pub status: Status,
    pub due: Option<Date>,
    pub priority: Option<i64>,
    pub tags: BTreeSet<String>,
    pub assignees: BTreeSet<IdentityId>,
    pub custom: BTreeMap<FieldName, FieldValue>,
}

macro_rules! define_events {
    ($($tag:literal => $variant:ident { $($field:ident: $ty:ty),+ }),+ $(,)?) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        pub enum Event {
            $(#[serde(rename = $tag)] $variant { $($field: $ty),+ }),+
        }

        impl Event {
            pub(crate) fn knows_variant(tag: &str) -> bool {
                matches!(tag, $($tag)|+)
            }
        }
    };
}

define_events! {
    "task_created" => TaskCreated { title: String, fields: InitialFields },
    "task_field_set" => TaskFieldSet { task: TaskId, field: FieldName, value: FieldValue },
    "tag_added" => TagAdded { task: TaskId, tag: String },
    "tag_removed" => TagRemoved { task: TaskId, tag: String },
    "assignee_added" => AssigneeAdded { task: TaskId, who: IdentityId },
    "assignee_removed" => AssigneeRemoved { task: TaskId, who: IdentityId },
    "edge_added" => EdgeAdded { from: TaskId, kind: EdgeKind, to: TaskId },
    "edge_removed" => EdgeRemoved { from: TaskId, kind: EdgeKind, to: TaskId },
    "comment_added" => CommentAdded { task: TaskId, text: String },
    "task_deleted" => TaskDeleted { task: TaskId },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InvalidEvent {
    #[error("task title is empty")]
    EmptyTitle,
    #[error("invalid field name {field:?}: {reason}")]
    FieldName {
        field: String,
        reason: InvalidFieldName,
    },
    #[error("incorrect value type for built-in field {0:?}")]
    FieldType(FieldName),
    #[error("built-in field {0:?} cannot appear in initial custom fields")]
    BuiltInCustomField(FieldName),
    #[error("invalid tag {0:?}")]
    Tag(String),
    #[error("edge connects task {0} to itself")]
    SelfEdge(TaskId),
}

impl Event {
    pub fn validate(&self) -> Result<(), InvalidEvent> {
        match self {
            Self::TaskCreated { title, fields } => {
                validate_title(title)?;
                for tag in &fields.tags {
                    validate_tag(tag)?;
                }
                for field in fields.custom.keys() {
                    validate_field_name(field)?;
                    if !field.is_custom() {
                        return Err(InvalidEvent::BuiltInCustomField(field.clone()));
                    }
                }
                Ok(())
            }
            Self::TaskFieldSet { field, value, .. } => {
                validate_field_name(field)?;
                match (field.as_str(), value) {
                    ("title", FieldValue::Text(title)) => validate_title(title),
                    ("description", FieldValue::Text(_) | FieldValue::Unset)
                    | ("status", FieldValue::Status(_))
                    | ("due", FieldValue::Date(_) | FieldValue::Unset)
                    | ("priority", FieldValue::Integer(_) | FieldValue::Unset) => Ok(()),
                    _ if field.is_custom() => Ok(()),
                    _ => Err(InvalidEvent::FieldType(field.clone())),
                }
            }
            Self::TagAdded { tag, .. } | Self::TagRemoved { tag, .. } => validate_tag(tag),
            Self::EdgeAdded { from, to, .. } | Self::EdgeRemoved { from, to, .. } if from == to => {
                Err(InvalidEvent::SelfEdge(*from))
            }
            _ => Ok(()),
        }
    }
}

fn validate_field_name(field: &FieldName) -> Result<(), InvalidEvent> {
    field.validate().map_err(|reason| InvalidEvent::FieldName {
        field: field.as_str().into(),
        reason,
    })
}

fn validate_title(title: &str) -> Result<(), InvalidEvent> {
    if title.is_empty() {
        return Err(InvalidEvent::EmptyTitle);
    }
    Ok(())
}

fn validate_tag(tag: &str) -> Result<(), InvalidEvent> {
    if !valid_name(tag, b"./") {
        return Err(InvalidEvent::Tag(tag.into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_creation_preserves_initial_fields() {
        let event = Event::TaskCreated {
            title: "Repair the garden gate".into(),
            fields: InitialFields {
                description: Some("Replace the lower hinge".into()),
                status: Status::Doing,
                due: Date::from_ymd_opt(2026, 12, 1),
                priority: Some(2),
                tags: BTreeSet::from(["garden".into()]),
                assignees: BTreeSet::from([IdentityId::from_bytes([25; 32])]),
                custom: BTreeMap::from([(
                    FieldName::new("estimate").unwrap(),
                    FieldValue::Integer(3),
                )]),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["task_created"]["fields"]["due"], "2026-12-01");
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
    }

    #[test]
    fn initial_fields_default_to_an_open_task() {
        let fields = InitialFields::default();
        assert_eq!(fields.status, Status::Todo);
        assert!(fields.tags.is_empty());
        assert!(fields.assignees.is_empty());
        assert!(fields.custom.is_empty());
    }
}
