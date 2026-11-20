use pepino_core::TaskId;
use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{term}: {kind}")]
pub struct QueryError {
    pub term: String,
    pub kind: QueryErrorKind,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    #[error("no current identity for me")]
    MissingIdentity,
    #[error("invalid task ID prefix {prefix:?} (expected 4 to 64 hexadecimal digits)")]
    InvalidTaskPrefix { prefix: String },
    #[error("unknown task ID prefix {prefix:?}")]
    UnknownTaskPrefix { prefix: String },
    #[error("ambiguous task ID prefix {prefix:?} ({count} matches): {candidates:?}")]
    AmbiguousTaskPrefix {
        prefix: String,
        count: u64,
        candidates: Vec<TaskId>,
    },
}
