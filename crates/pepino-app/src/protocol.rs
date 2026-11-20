use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Date, EdgeKind, EventRecord, FieldValue, FilterDiagnostic, FilterInput, Hlc, IdentityRecord,
    NamedRecord, SavedView, Status, StatusCounts,
};

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
    GetTask {
        task: String,
    },
    Tree {
        task: Option<String>,
    },
    Deps {
        task: String,
    },
    Status,
    Log {
        task: Option<String>,
        limit: Option<usize>,
    },
    ListViews,
    ListTasks {
        filter: FilterInput,
        sort: Option<String>,
        limit: Option<usize>,
        today: Date,
    },
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
    SaveView {
        name: String,
        filter: FilterInput,
    },
    DeleteView {
        name: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
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
        tasks: Vec<TaskSummary>,
        edges: Vec<EdgeEffect>,
    },
    Task {
        task: Box<TaskRecord>,
        #[serde(default)]
        edges: Vec<EdgeEffect>,
    },
    Tree {
        nodes: Vec<TreeNode>,
    },
    Deps {
        task: Box<TaskSummary>,
        blockers: Vec<DependencyRecord>,
        dependents: Vec<DependencyRecord>,
    },
    Status {
        identity: IdentityRecord,
        device: NamedRecord,
        board: NamedRecord,
        data_dir: String,
        counts: StatusCounts,
        protocol_version: u32,
    },
    Log {
        titles: BTreeMap<String, Option<String>>,
        events: Vec<EventRecord>,
        unordered: Vec<EventRecord>,
    },
    ViewSaved {
        view: SavedView,
        replaced: bool,
    },
    ViewDeleted {
        view: SavedView,
    },
    Views {
        views: Vec<SavedView>,
    },
    Tasks {
        tasks: Vec<TaskSummary>,
        total: usize,
        filter: String,
        resolved_dates: BTreeMap<String, Date>,
        sort: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TreeNode {
    pub depth: usize,
    pub task: TaskSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DependencyRecord {
    pub distance: usize,
    pub task: TaskSummary,
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
    #[serde(flatten)]
    pub summary: TaskSummary,
    pub comments: Vec<CommentRecord>,
}

impl std::ops::Deref for TaskRecord {
    type Target = TaskSummary;

    fn deref(&self) -> &Self::Target {
        &self.summary
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSummary {
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
    pub comment_count: u64,
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
    #[error("{reference} at {field} was not found")]
    NotFound {
        field: String,
        #[serde(flatten)]
        reference: MissingReference,
    },
    #[error("task reference {task} at {field} matches {count} tasks")]
    AmbiguousId {
        field: String,
        task: String,
        count: u64,
        candidates: Vec<String>,
    },
    #[error("invalid {field}: {reason}")]
    InvalidInput {
        field: String,
        reason: String,
        #[serde(flatten)]
        diagnostic: Option<Box<FilterDiagnostic>>,
    },
    #[error("{reference} at {field}")]
    Conflict {
        field: String,
        #[serde(flatten)]
        reference: ConflictReference,
    },
    #[error("another pepino process holds the data directory")]
    Busy,
    #[error("storage error: {reason}")]
    Storage { retryable: bool, reason: String },
    #[error("protocol version {requested} is unsupported (supported version {supported})")]
    UnsupportedVersion { requested: u32, supported: u32 },
    #[error("write was committed: {reason}")]
    WriteCommitted { reason: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MissingReference {
    Task(String),
    View(String),
    Board(String),
}

impl std::fmt::Display for MissingReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task(task) => write!(f, "task {task}"),
            Self::View(view) => write!(f, "view {view}"),
            Self::Board(board) => write!(f, "board {board}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ConflictReference {
    Task {
        task: String,
    },
    View {
        view: String,
        referenced_by: Vec<String>,
    },
}

impl std::fmt::Display for ConflictReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task { task } => write!(f, "task {task} is deleted"),
            Self::View {
                view,
                referenced_by,
            } => write!(
                f,
                "view {view} is referenced by {}",
                referenced_by.join(", ")
            ),
        }
    }
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
