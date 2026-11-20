use std::fmt;

use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("filter term {term:?} at {position}: {kind}")]
pub struct ParseError {
    pub term: String,
    pub position: Position,
    pub kind: ErrorKind,
}

/// Zero-based UTF-8 byte position in the original string or argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Position {
    Byte(usize),
    Argument { index: usize, byte: usize },
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Byte(index) => write!(f, "byte {index}"),
            Self::Argument { index, byte } => write!(f, "argument {index}, byte {byte}"),
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
    #[error(
        "invalid saved view name {name:?}. View names cannot contain spaces and must match [a-z][a-z0-9_-]* within 64 bytes"
    )]
    InvalidView { name: String },
    #[error("unknown filter key {value:?}. Use {suggested:?}")]
    UnknownKey {
        value: String,
        suggested: &'static str,
    },
    #[error("empty value; supply a value for each alternative")]
    EmptyValue,
    #[error("comparisons are unavailable for {key:?}; use {key}:value for equality")]
    UnsupportedComparison { key: String },
    #[error(
        "unknown status {value:?}. Use {}",
        super::keywords::accepted(super::keywords::STATUSES)
    )]
    UnknownStatus { value: String },
    #[error("invalid tag {value:?}. Tags must match [a-z][a-z0-9_./-]*")]
    InvalidTag { value: String },
    #[error(
        "unknown state {value:?}. Use {}",
        super::keywords::accepted(super::State::WORDS)
    )]
    UnknownState { value: String },
    #[error(
        "unsupported presence field {field:?}. Use {} or a custom field",
        super::keywords::accepted(super::PresenceField::WORDS)
    )]
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
