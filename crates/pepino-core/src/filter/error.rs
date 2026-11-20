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
    #[error("unterminated quote. Add a closing double quote")]
    UnterminatedQuote,
    #[error("empty term. Supply a search term")]
    EmptyTerm,
    #[error("empty view reference. Supply a saved view name after '@'")]
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
    #[error("empty value. Supply a value for each alternative")]
    EmptyValue,
    #[error("comparisons are unavailable for {key:?}. Use {key}:value for equality")]
    UnsupportedComparison { key: String },
    #[error(
        "unknown status {value:?}. Use {}",
        super::keywords::accepted(super::keywords::STATUSES, None)
    )]
    UnknownStatus { value: String },
    #[error("invalid tag {value:?}. Tags must match [a-z][a-z0-9_./-]*")]
    InvalidTag { value: String },
    #[error(
        "unknown state {value:?}. Use {}",
        super::keywords::accepted(super::State::WORDS, None)
    )]
    UnknownState { value: String },
    #[error(
        "unsupported presence field {field:?}. Use {}",
        super::keywords::accepted(super::PresenceField::WORDS, Some("a custom field"))
    )]
    UnsupportedPresence { field: String },
    #[error(
        "invalid date {value:?}. Use YYYY-MM-DD, today, tomorrow, yesterday, a weekday, or an offset such as +3d"
    )]
    InvalidDate { value: String },
    #[error("invalid integer {value:?}. Enter a signed 64-bit integer")]
    InvalidInteger { value: String },
    #[error("date {value:?} is outside the supported calendar range")]
    DateOutOfRange { value: String },
    #[error("keywords are lowercase. Use {lowercase:?} for {value:?}")]
    LowercaseKeyword { value: String, lowercase: String },
}

impl ErrorKind {
    pub fn hint(&self) -> Option<String> {
        use super::keywords::{STATUSES, accepted};

        let hint = match self {
            Self::UnterminatedQuote => "Add a closing double quote".into(),
            Self::EmptyTerm => "Supply a search term".into(),
            Self::EmptyView => "Supply a saved view name after '@'".into(),
            Self::InvalidView { .. } => {
                "View names cannot contain spaces and must match [a-z][a-z0-9_-]* within 64 bytes"
                    .into()
            }
            Self::UnknownKey { suggested, .. } => format!("Use {suggested:?}"),
            Self::EmptyValue => "Supply a value for each alternative".into(),
            Self::UnsupportedComparison { key } => format!("Use {key}:value for equality"),
            Self::UnknownStatus { .. } => format!("Use {}", accepted(STATUSES, None)),
            Self::InvalidTag { .. } => "Tags must match [a-z][a-z0-9_./-]*".into(),
            Self::UnknownState { .. } => format!("Use {}", accepted(super::State::WORDS, None)),
            Self::UnsupportedPresence { .. } => format!(
                "Use {}",
                accepted(super::PresenceField::WORDS, Some("a custom field"))
            ),
            Self::InvalidDate { .. } => {
                "Use YYYY-MM-DD, today, tomorrow, yesterday, a weekday, or an offset such as +3d"
                    .into()
            }
            Self::InvalidInteger { .. } => "Enter a signed 64-bit integer".into(),
            Self::DateOutOfRange { .. } => return None,
            Self::LowercaseKeyword { value, lowercase } => {
                format!("Use {lowercase:?} for {value:?}")
            }
        };
        Some(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_hint_can_be_read_without_formatting_the_error() {
        let error = ErrorKind::InvalidInteger {
            value: "3.5".into(),
        };
        assert_eq!(
            error.hint().as_deref(),
            Some("Enter a signed 64-bit integer")
        );
    }
}
