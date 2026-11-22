use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Date, EdgeKind, EventRecord, FieldValue, FilterDiagnostic, FilterInput, FilterWarning, Hlc,
    IdentityRecord, NamedRecord, SavedView, Status, StatusCounts,
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
    IdentityShow,
    Log {
        task: Option<String>,
        limit: Option<usize>,
    },
    PreviewDelete {
        tasks: Vec<String>,
    },
    ListViews,
    BoardList,
    BoardShow {
        board: Option<String>,
    },
    ListOpenTasks {
        filter: FilterInput,
        sort: Option<String>,
        limit: Option<usize>,
        today: Date,
    },
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
    IdentityRename {
        name: String,
    },
    DeviceRename {
        device: Option<String>,
        name: String,
    },
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
    ApplyToTasks {
        tasks: Vec<String>,
        change: TaskChange,
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
    BoardUse {
        board: String,
    },
    BoardRename {
        board: String,
        name: String,
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
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum TaskChange {
    SetFields {
        changes: BTreeMap<String, FieldValue>,
    },
    Delete,
    Reopen,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    Boards {
        boards: Vec<crate::BoardInfo>,
    },
    Board {
        board: crate::BoardInfo,
    },
    DeletePreview {
        targets: Vec<String>,
        tasks: Vec<TaskSummary>,
        top_level: Vec<TaskSummary>,
        unblocked: Vec<TaskSummary>,
    },
    BatchWrite {
        outcome: Box<Response>,
        changed_targets: Vec<String>,
    },
    Write {
        changed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        created: Option<String>,
        tasks: Vec<TaskSummary>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        targets: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        deleted: Vec<TaskSummary>,
        edges: Vec<EdgeEffect>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        activated: Vec<EdgeEffect>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        top_level: Vec<TaskSummary>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unblocked: Vec<TaskSummary>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
    },
    Task {
        task: Box<TaskRecord>,
        #[serde(default)]
        edges: Vec<EdgeEffect>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        relation_titles: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        assignee_names: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
    },
    Tree {
        nodes: Vec<TreeNode>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
    },
    Deps {
        task: Box<TaskSummary>,
        blockers: Vec<DependencyRecord>,
        dependents: Vec<DependencyRecord>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
    },
    Identity {
        identity: IdentityRecord,
    },
    RosterDevice {
        device: crate::RosterDevice,
    },
    Status {
        identity: Box<IdentityRecord>,
        device: NamedRecord,
        board: NamedRecord,
        data_dir: String,
        credentials_file: String,
        network: crate::NetworkStatus,
        counts: StatusCounts,
        protocol_version: u32,
    },
    Log {
        titles: BTreeMap<String, Option<String>>,
        events: Vec<EventRecord>,
        unordered: Vec<EventRecord>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        not_imported: Vec<String>,
    },
    Tasks {
        tasks: Vec<TaskSummary>,
        total: usize,
        filter: String,
        resolved_dates: BTreeMap<String, Date>,
        sort: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<FilterWarning>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        constrains_status: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        names: BTreeMap<String, String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<crate::LocalBoard>,
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
    NotInitialized,
    DeviceRequestPending,
    IncompatibleLocalState,
    PermissionDenied,
    DeviceDirectory,
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
            Self::NotInitialized => "not_initialized",
            Self::DeviceRequestPending => "device_request_pending",
            Self::IncompatibleLocalState => "incompatible_local_state",
            Self::PermissionDenied => "permission_denied",
            Self::DeviceDirectory => "device_directory",
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
    #[error("{}", reference.message(field))]
    NotFound {
        field: String,
        #[serde(flatten)]
        reference: MissingReference,
    },
    #[error("board reference {board:?} at {field} is ambiguous")]
    AmbiguousBoard {
        field: String,
        board: String,
        candidates: Vec<crate::LocalBoard>,
    },
    #[error("ambiguous person {reference:?}")]
    AmbiguousPerson {
        reference: String,
        field: String,
        candidates: Vec<String>,
        identity_ids: Vec<String>,
    },
    #[error("device reference {reference:?} matches several devices")]
    AmbiguousDevice {
        reference: String,
        candidates: Vec<String>,
    },
    #[error("task reference {task} at {field} matches {count} tasks")]
    AmbiguousId {
        field: String,
        task: String,
        count: u64,
        candidates: Vec<String>,
        #[serde(flatten)]
        diagnostic: Option<Box<FilterDiagnostic>>,
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
    #[error("{directory} is not initialized; run ppn init")]
    NotInitialized { directory: String },
    #[error("Device request pending: run `ppn device join`")]
    DeviceRequestPending,
    #[error(
        "local state version {version} in {directory} cannot be upgraded because its device and board IDs are not real keys and spaces. Move the directory aside and run ppn init."
    )]
    IncompatibleLocalState { version: u64, directory: String },
    #[error("permission denied for {path}: {reason}")]
    PermissionDenied { path: String, reason: String },
    #[error(
        "This data directory is a device; use a separate data directory for a relay: {directory}"
    )]
    DeviceDirectory { directory: String },
    #[error("This device was revoked from identity {identity} on {at} by {by}.")]
    DeviceRevoked {
        identity: String,
        at: chrono::DateTime<chrono::Utc>,
        by: pepino_core::DeviceId,
    },
    #[error("This device has requested to leave. Another active device must complete the removal.")]
    DeviceLeaving,
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
    TaskOnOtherBoards {
        task: String,
        alternate_boards: Vec<crate::LocalBoard>,
    },
    DeletedTask {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    View(String),
    Board(String),
    Device(String),
    Person(String),
}

impl MissingReference {
    fn message(&self, field: &str) -> String {
        if matches!(self, Self::DeletedTask { .. }) {
            format!("{self} at {field}")
        } else {
            format!("{self} at {field} was not found")
        }
    }
}

impl std::fmt::Display for MissingReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task(task) | Self::TaskOnOtherBoards { task, .. } => write!(f, "task {task}"),
            Self::DeletedTask {
                task,
                title: Some(title),
            } => write!(f, "task {task} {title:?} was deleted"),
            Self::DeletedTask { task, title: None } => write!(f, "task {task} was deleted"),
            Self::View(view) => write!(f, "view {view}"),
            Self::Board(board) => write!(f, "board {board}"),
            Self::Device(device) => write!(f, "device {device}"),
            Self::Person(value) => write!(f, "Person {value:?}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum ConflictReference {
    Task {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Relation {
        reason: String,
    },
    View {
        view: String,
        referenced_by: Vec<String>,
    },
}

impl std::fmt::Display for ConflictReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task { task, .. } => write!(f, "task {task} is deleted"),
            Self::Relation { reason } => write!(f, "{reason}"),
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
            Self::NotInitialized { .. } => AppErrorKind::NotInitialized,
            Self::DeviceRequestPending => AppErrorKind::DeviceRequestPending,
            Self::IncompatibleLocalState { .. } => AppErrorKind::IncompatibleLocalState,
            Self::PermissionDenied { .. } | Self::DeviceRevoked { .. } | Self::DeviceLeaving => {
                AppErrorKind::PermissionDenied
            }
            Self::DeviceDirectory { .. } => AppErrorKind::DeviceDirectory,
            Self::AmbiguousId { .. }
            | Self::AmbiguousBoard { .. }
            | Self::AmbiguousPerson { .. }
            | Self::AmbiguousDevice { .. } => AppErrorKind::AmbiguousId,
            Self::InvalidInput { .. } => AppErrorKind::InvalidInput,
            Self::Conflict { .. } => AppErrorKind::Conflict,
            Self::Busy => AppErrorKind::Busy,
            Self::Storage { .. } => AppErrorKind::Storage,
            Self::UnsupportedVersion { .. } => AppErrorKind::UnsupportedVersion,
            Self::WriteCommitted { .. } => AppErrorKind::WriteCommitted,
        }
    }
}
