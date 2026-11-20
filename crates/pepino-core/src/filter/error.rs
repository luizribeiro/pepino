use std::fmt;

use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("filter term {term:?} at {position}: {kind}")]
pub struct ParseError {
    pub term: String,
    pub position: Position,
    pub kind: ErrorKind,
}

/// Zero-based term position in the original input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Position {
    Byte(usize),
    Argument(usize),
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Byte(index) => write!(f, "byte {index}"),
            Self::Argument(index) => write!(f, "argument {index}"),
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ErrorKind {
    #[error("unterminated quote; add a closing double quote")]
    UnterminatedQuote,
    #[error("empty term; supply a search term")]
    EmptyTerm,
    #[error("empty view reference; supply a saved view name after '@'")]
    EmptyView,
    #[error("empty key; supply a field name before ':'")]
    EmptyKey,
    #[error("empty value; supply a value for each alternative")]
    EmptyValue,
    #[error("comparisons are unavailable for {key:?}; use {key}:value for equality")]
    UnsupportedComparison { key: String },
    #[error("unknown status {value:?}; use todo, doing, done, or cancelled")]
    UnknownStatus { value: String },
    #[error("unknown state {value:?}; use blocked, ready, open, or closed")]
    UnknownState { value: String },
    #[error("unsupported presence field {field:?}; use due, parent, assignee, or a custom field")]
    UnsupportedPresence { field: String },
    #[error(
        "invalid date {value:?}; use YYYY-MM-DD, today, tomorrow, yesterday, a weekday, or an offset such as +3d"
    )]
    InvalidDate { value: String },
    #[error("date {value:?} is outside the supported calendar range")]
    DateOutOfRange { value: String },
    #[error("keywords are lowercase; use {lowercase:?} for {value:?}")]
    LowercaseKeyword { value: String, lowercase: String },
}
