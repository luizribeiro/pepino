use std::collections::{BTreeMap, BTreeSet};

use pepino_core::InitialFields;
use serde::{Deserialize, Serialize};

use crate::{AppError, Date, EdgeKind, FieldValue, Hlc, Status};

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
