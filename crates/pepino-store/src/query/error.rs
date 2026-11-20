use pepino_core::{IdentityId, TaskId};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{}{term}: {kind}", view_location(.view_chain))]
pub struct QueryError {
    pub term: String,
    pub term_index: usize,
    pub kind: QueryErrorKind,
    /// Saved views entered before reaching the offending term, outermost first.
    pub view_chain: Vec<String>,
}

fn view_location(chain: &[String]) -> String {
    if chain.is_empty() {
        String::new()
    } else {
        format!("in @{}: ", chain.join(" -> @"))
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    #[error("text search requires a board snapshot loaded with comments")]
    CommentsRequired,
    #[error("date {expression:?} is outside the supported calendar range")]
    DateOutOfRange { expression: String },
    #[error("no current identity for me")]
    MissingIdentity,
    #[error("unknown person {text:?}")]
    UnknownPerson { text: String },
    #[error("ambiguous person {text:?}: {candidates:?}")]
    AmbiguousPerson {
        text: String,
        candidates: Vec<IdentityId>,
    },
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
    #[error("negated saved view @{name} selects boards")]
    NegatedViewWithBoards { name: String },
    #[error("unknown saved view {name:?}")]
    UnknownView { name: String },
    #[error("saved view cycle: {}", .chain.join(" -> "))]
    ViewCycle { chain: Vec<String> },
}
