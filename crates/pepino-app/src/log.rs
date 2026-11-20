use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{DecodeOutcome, Event, InitialFields};
use pepino_store::{EventClassification as StoredClassification, HistoryEvent};
use serde::{Deserialize, Serialize};

use crate::app::store_error;
use crate::projection::resolve;
use crate::{App, AppError, Date, EdgeKind, FieldValue, Hlc, Response, Status};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventRecord {
    pub id: String,
    pub author: Option<String>,
    pub hlc: Option<Hlc>,
    pub classification: EventClassification,
    pub kind: String,
    pub change: EventChange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventClassification {
    Applied,
    UnknownVersion,
    UnknownEvent,
    UnknownValue,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventChange {
    Creation {
        title: String,
        fields: InitialTaskFields,
    },
    Field {
        task: String,
        field: String,
        value: FieldValue,
    },
    Tag {
        task: String,
        tag: String,
    },
    Assignee {
        task: String,
        who: String,
    },
    Edge {
        from: String,
        kind: EdgeKind,
        to: String,
    },
    Comment {
        task: String,
        text: String,
    },
    Deletion {
        task: String,
    },
    Skipped {
        tasks: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitialTaskFields {
    pub description: Option<String>,
    pub status: Status,
    pub due: Option<Date>,
    pub priority: Option<i64>,
    pub tags: BTreeSet<String>,
    pub assignees: BTreeSet<String>,
    pub fields: BTreeMap<String, FieldValue>,
}

impl App {
    pub(super) async fn log(
        &self,
        reference: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Response, AppError> {
        let task = if let Some(reference) = reference {
            Some(resolve(&self.board().await?, "task", reference)?.id)
        } else {
            None
        };
        let events = self
            .store
            .history(self.local.board_id, task, limit)
            .await
            .map_err(store_error)?
            .into_iter()
            .map(EventRecord::try_from)
            .collect::<Result<_, _>>()?;
        Ok(Response::Log { events })
    }
}

impl From<StoredClassification> for EventClassification {
    fn from(value: StoredClassification) -> Self {
        match value {
            StoredClassification::Applied => Self::Applied,
            StoredClassification::UnknownVersion => Self::UnknownVersion,
            StoredClassification::UnknownEvent => Self::UnknownEvent,
            StoredClassification::UnknownValue => Self::UnknownValue,
            StoredClassification::Invalid => Self::Invalid,
        }
    }
}

impl TryFrom<HistoryEvent> for EventRecord {
    type Error = AppError;

    fn try_from(record: HistoryEvent) -> Result<Self, Self::Error> {
        let decoded = &record.decoded;
        let (kind, version, reason) = match decoded {
            DecodeOutcome::UnknownVersion { version, .. } => ("unknown", Some(*version), None),
            DecodeOutcome::UnknownEvent { variant, .. }
            | DecodeOutcome::UnknownValue { variant, .. } => (variant.as_str(), None, None),
            DecodeOutcome::InvalidEvent { reason, .. } => {
                ("unknown", None, Some(reason.to_string()))
            }
            _ => ("unknown", None, None),
        };
        let (kind, change) = match record.event {
            Some(event) => (event.kind(), EventChange::try_from(event)?),
            None => (
                kind,
                EventChange::Skipped {
                    tasks: record.tasks.into_iter().map(|id| id.to_string()).collect(),
                    version,
                },
            ),
        };
        Ok(Self {
            id: decoded.id().to_string(),
            author: decoded.author().map(|id| id.to_string()),
            hlc: decoded.hlc().map(Into::into),
            classification: record.classification.into(),
            kind: kind.into(),
            change,
            reason,
        })
    }
}

impl TryFrom<Event> for EventChange {
    type Error = AppError;

    fn try_from(value: Event) -> Result<Self, Self::Error> {
        Ok(match value {
            Event::TaskCreated { title, fields } => Self::Creation {
                title,
                fields: fields.try_into()?,
            },
            Event::TaskFieldSet { task, field, value } => Self::Field {
                task: task.to_string(),
                field: field.as_str().into(),
                value: value.try_into()?,
            },
            Event::TagAdded { task, tag } | Event::TagRemoved { task, tag } => Self::Tag {
                task: task.to_string(),
                tag,
            },
            Event::AssigneeAdded { task, who } | Event::AssigneeRemoved { task, who } => {
                Self::Assignee {
                    task: task.to_string(),
                    who: who.to_string(),
                }
            }
            Event::EdgeAdded { from, kind, to } | Event::EdgeRemoved { from, kind, to } => {
                Self::Edge {
                    from: from.to_string(),
                    kind: kind.try_into()?,
                    to: to.to_string(),
                }
            }
            Event::CommentAdded { task, text } => Self::Comment {
                task: task.to_string(),
                text,
            },
            Event::TaskDeleted { task } => Self::Deletion {
                task: task.to_string(),
            },
        })
    }
}

impl TryFrom<InitialFields> for InitialTaskFields {
    type Error = AppError;

    fn try_from(value: InitialFields) -> Result<Self, Self::Error> {
        Ok(Self {
            description: value.description,
            status: value.status.try_into()?,
            due: value.due.map(Into::into),
            priority: value.priority,
            tags: value.tags,
            assignees: value
                .assignees
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            fields: value
                .custom
                .into_iter()
                .map(|(name, value)| Ok((name.as_str().into(), value.try_into()?)))
                .collect::<Result<_, AppError>>()?,
        })
    }
}
