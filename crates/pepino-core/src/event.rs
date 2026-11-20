use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{
    Date, EdgeKind, FieldName, FieldValue, IdentityId, InvalidFieldName, Status, TaskId,
    field::valid_name,
};

const MAX_TITLE_BYTES: usize = 1024;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_TAG_BYTES: usize = 64;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitialFields {
    pub description: Option<String>,
    pub status: Status,
    pub due: Option<Date>,
    pub priority: Option<i64>,
    pub tags: BTreeSet<String>,
    pub assignees: BTreeSet<IdentityId>,
    #[serde(deserialize_with = "deserialize_custom_fields")]
    pub custom: BTreeMap<FieldName, FieldValue>,
}

macro_rules! define_events {
    ($($tag:literal => $variant:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),+ }),+ $(,)?) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
        #[non_exhaustive]
        pub enum Event {
            $(#[serde(rename = $tag)] $variant { $($(#[$attr])* $field: $ty),+ }),+
        }

        impl Event {
            pub const fn kind(&self) -> &'static str {
                match self {
                    $(Self::$variant { .. } => $tag),+
                }
            }

            pub(crate) fn knows_variant(tag: &str) -> bool {
                matches!(tag, $($tag)|+)
            }
        }
    };
}

define_events! {
    "task_created" => TaskCreated { title: String, fields: InitialFields },
    "task_field_set" => TaskFieldSet { task: TaskId, #[serde(deserialize_with = "deserialize_event_field")] field: FieldName, value: FieldValue },
    "tag_added" => TagAdded { task: TaskId, tag: String },
    "tag_removed" => TagRemoved { task: TaskId, tag: String },
    "assignee_added" => AssigneeAdded { task: TaskId, who: IdentityId },
    "assignee_removed" => AssigneeRemoved { task: TaskId, who: IdentityId },
    "edge_added" => EdgeAdded { from: TaskId, kind: EdgeKind, to: TaskId },
    "edge_removed" => EdgeRemoved { from: TaskId, kind: EdgeKind, to: TaskId },
    "comment_added" => CommentAdded { task: TaskId, text: String },
    "task_deleted" => TaskDeleted { task: TaskId },
}

// Event::validate classifies a wire name with invalid syntax as an invalid event.
fn deserialize_event_field<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<FieldName, D::Error> {
    String::deserialize(deserializer).map(FieldName::from_event_name)
}

fn deserialize_custom_fields<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<FieldName, FieldValue>, D::Error> {
    Ok(BTreeMap::<String, FieldValue>::deserialize(deserializer)?
        .into_iter()
        .map(|(name, value)| (FieldName::from_event_name(name), value))
        .collect())
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InvalidEvent {
    #[error("task title is empty")]
    EmptyTitle,
    #[error("{kind} exceeds the {max_bytes}-byte limit")]
    TextTooLong {
        kind: &'static str,
        max_bytes: usize,
    },
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
    /// Returns the target task. For edges, this is the source task.
    pub const fn task(&self) -> Option<TaskId> {
        match self {
            Self::TaskCreated { .. } => None,
            Self::TaskFieldSet { task, .. }
            | Self::TagAdded { task, .. }
            | Self::TagRemoved { task, .. }
            | Self::AssigneeAdded { task, .. }
            | Self::AssigneeRemoved { task, .. }
            | Self::CommentAdded { task, .. }
            | Self::TaskDeleted { task } => Some(*task),
            Self::EdgeAdded { from, .. } | Self::EdgeRemoved { from, .. } => Some(*from),
        }
    }

    /// Returns the task IDs in the event, with the source before the destination for edges.
    /// Creation has no task ID in the payload. See [`crate::Envelope::created_task`].
    pub fn referenced_tasks(&self) -> impl Iterator<Item = TaskId> + use<> {
        let destination = match self {
            Self::EdgeAdded { to, .. } | Self::EdgeRemoved { to, .. } => Some(*to),
            _ => None,
        };
        [self.task(), destination].into_iter().flatten()
    }

    /// Checks the event against the validity rules of its wire format.
    ///
    /// Validity rules are frozen per format version. Adding a built-in field
    /// or tightening a validity rule requires a new format version.
    pub fn validate(&self) -> Result<(), InvalidEvent> {
        match self {
            Self::TaskCreated { title, fields } => {
                validate_title(title)?;
                if let Some(description) = &fields.description {
                    validate_length(description, "description", MAX_TEXT_BYTES)?;
                }
                for tag in &fields.tags {
                    validate_tag(tag)?;
                }
                for (field, value) in &fields.custom {
                    validate_field_name(field)?;
                    if !field.is_custom() {
                        return Err(InvalidEvent::BuiltInCustomField(field.clone()));
                    }
                    if let FieldValue::Text(text) = value {
                        validate_length(text, "field text", MAX_TEXT_BYTES)?;
                    }
                }
                Ok(())
            }
            Self::TaskFieldSet { field, value, .. } => {
                validate_field_name(field)?;
                match (field.as_str(), value) {
                    ("title", FieldValue::Text(title)) => validate_title(title),
                    ("description", FieldValue::Text(description)) => {
                        validate_length(description, "description", MAX_TEXT_BYTES)
                    }
                    ("description", FieldValue::Unset)
                    | ("status", FieldValue::Status(_))
                    | ("due", FieldValue::Date(_) | FieldValue::Unset)
                    | ("priority", FieldValue::Integer(_) | FieldValue::Unset) => Ok(()),
                    (_, FieldValue::Text(text)) if field.is_custom() => {
                        validate_length(text, "field text", MAX_TEXT_BYTES)
                    }
                    _ if field.is_custom() => Ok(()),
                    _ => Err(InvalidEvent::FieldType(field.clone())),
                }
            }
            Self::TagAdded { tag, .. } | Self::TagRemoved { tag, .. } => validate_tag(tag),
            Self::CommentAdded { text, .. } => validate_length(text, "comment", MAX_TEXT_BYTES),
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
    validate_length(title, "title", MAX_TITLE_BYTES)
}

fn validate_tag(tag: &str) -> Result<(), InvalidEvent> {
    if tag.len() > MAX_TAG_BYTES || !valid_name(tag, b"./") {
        return Err(InvalidEvent::Tag(tag.into()));
    }
    Ok(())
}

fn validate_length(text: &str, kind: &'static str, max_bytes: usize) -> Result<(), InvalidEvent> {
    if text.len() > max_bytes {
        return Err(InvalidEvent::TextTooLong { kind, max_bytes });
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
