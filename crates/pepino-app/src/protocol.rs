use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Date, EdgeKind, FieldValue, Hlc, Status};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Request {
    Read(ReadRequest),
    Write(WriteRequest),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ReadRequest {
    GetTask { task: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum WriteRequest {
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    Write {
        changed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        created: Option<String>,
        tasks: Vec<TaskRecord>,
        edges: Vec<EdgeEffect>,
    },
    Task {
        task: Box<TaskRecord>,
        #[serde(default)]
        edges: Vec<EdgeEffect>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EdgeEffect {
    pub from: String,
    pub kind: EdgeKind,
    pub to: String,
    pub effective: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<EdgeExclusionReason>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EdgeExclusionReason {
    MissingEndpoint,
    SupersededParent,
    Cycle,
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
    Busy,
    Storage,
    UnsupportedVersion,
    WriteCommitted,
}

impl AppErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::AmbiguousId => "ambiguous_id",
            Self::InvalidInput => "invalid_input",
            Self::Conflict => "conflict",
            Self::Busy => "busy",
            Self::Storage => "storage",
            Self::UnsupportedVersion => "unsupported_version",
            Self::WriteCommitted => "write_committed",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppError {
    #[error("task {task} at {field} was not found")]
    NotFound { field: String, task: String },
    #[error("task reference {task} at {field} matches {count} tasks")]
    AmbiguousId {
        field: String,
        task: String,
        count: u64,
        candidates: Vec<String>,
    },
    #[error("invalid {field}: {reason}")]
    InvalidInput { field: String, reason: String },
    #[error("task {task} at {field} is deleted")]
    Conflict { field: String, task: String },
    #[error("another pepino process holds the data directory")]
    Busy,
    #[error("storage error: {reason}")]
    Storage { retryable: bool, reason: String },
    #[error("protocol version {requested} is unsupported (supported version {supported})")]
    UnsupportedVersion { requested: u32, supported: u32 },
    #[error("write was committed: {reason}")]
    WriteCommitted { reason: String },
}

impl AppError {
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Busy
                | Self::Storage {
                    retryable: true,
                    ..
                }
        )
    }

    pub const fn kind(&self) -> AppErrorKind {
        match self {
            Self::NotFound { .. } => AppErrorKind::NotFound,
            Self::AmbiguousId { .. } => AppErrorKind::AmbiguousId,
            Self::InvalidInput { .. } => AppErrorKind::InvalidInput,
            Self::Conflict { .. } => AppErrorKind::Conflict,
            Self::Busy => AppErrorKind::Busy,
            Self::Storage { .. } => AppErrorKind::Storage,
            Self::UnsupportedVersion { .. } => AppErrorKind::UnsupportedVersion,
            Self::WriteCommitted { .. } => AppErrorKind::WriteCommitted,
        }
    }
}
