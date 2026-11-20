use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use pepino_core::{Date, EdgeKind, FieldValue, Hlc, Status};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request {
    AddTask {
        title: String,
        description: Option<String>,
        #[serde(default)]
        status: Status,
        due: Option<Date>,
        priority: Option<i64>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        assignees: Vec<String>,
        #[serde(default)]
        fields: BTreeMap<String, FieldValue>,
        parent: Option<String>,
        #[serde(default)]
        blocks: Vec<String>,
        #[serde(default)]
        blocked_by: Vec<String>,
    },
    SetFields {
        task: String,
        changes: BTreeMap<String, FieldValue>,
    },
    AddTags {
        task: String,
        tags: Vec<String>,
    },
    RemoveTags {
        task: String,
        tags: Vec<String>,
    },
    Assign {
        task: String,
        assignees: Vec<String>,
    },
    Unassign {
        task: String,
        assignees: Vec<String>,
    },
    Comment {
        task: String,
        text: String,
    },
    Link {
        from: String,
        kind: EdgeKind,
        to: String,
    },
    Unlink {
        from: String,
        kind: EdgeKind,
        to: String,
    },
    Delete {
        task: String,
    },
    GetTask {
        task: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    Write {
        changed: bool,
        task_ids: Vec<String>,
        tasks: Vec<TaskRecord>,
    },
    Task {
        task: Box<TaskRecord>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskRecord {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub due: Option<Date>,
    pub priority: Option<i64>,
    pub fields: BTreeMap<String, FieldValue>,
    pub tags: BTreeSet<String>,
    pub assignees: BTreeSet<String>,
    pub created: Hlc,
    pub updated: Hlc,
    pub author: String,
    pub parent: Option<String>,
    pub children: BTreeSet<String>,
    pub blocks: BTreeSet<String>,
    pub blocked_by: BTreeSet<String>,
    pub relates: BTreeSet<String>,
    pub blocked: bool,
    pub ready: bool,
    pub comments: Vec<CommentRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommentRecord {
    pub id: String,
    pub author: String,
    pub hlc: Hlc,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppErrorKind {
    NotFound,
    AmbiguousId,
    InvalidInput,
    Conflict,
    Storage,
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppError {
    #[error("task {task:?} was not found")]
    NotFound { task: String },
    #[error("task reference {task:?} matches {count} tasks")]
    AmbiguousId {
        task: String,
        count: u64,
        candidates: Vec<String>,
    },
    #[error("invalid {field}: {reason}")]
    InvalidInput { field: String, reason: String },
    #[error("task {task} is deleted")]
    Conflict { task: String },
    #[error("storage error: {reason}")]
    Storage { retryable: bool, reason: String },
}

impl AppError {
    pub const fn kind(&self) -> AppErrorKind {
        match self {
            Self::NotFound { .. } => AppErrorKind::NotFound,
            Self::AmbiguousId { .. } => AppErrorKind::AmbiguousId,
            Self::InvalidInput { .. } => AppErrorKind::InvalidInput,
            Self::Conflict { .. } => AppErrorKind::Conflict,
            Self::Storage { .. } => AppErrorKind::Storage,
        }
    }
}
