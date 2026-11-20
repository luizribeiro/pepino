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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    UnterminatedQuote,
    EmptyTerm,
    EmptyView,
    InvalidView {
        name: String,
    },
    UnknownKey {
        value: String,
        suggested: &'static str,
    },
    EmptyValue,
    UnsupportedComparison {
        key: String,
    },
    UnknownStatus {
        value: String,
    },
    InvalidTag {
        value: String,
    },
    UnknownState {
        value: String,
    },
    UnsupportedPresence {
        field: String,
    },
    InvalidDate {
        value: String,
    },
    InvalidInteger {
        value: String,
    },
    DateOutOfRange {
        value: String,
    },
    LowercaseKeyword {
        value: String,
        lowercase: String,
    },
}

impl ErrorKind {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::UnterminatedQuote => "unterminated quote".into(),
            Self::EmptyTerm => "empty term".into(),
            Self::EmptyView => "empty view reference".into(),
            Self::InvalidView { name } => format!("invalid saved view name {name:?}"),
            Self::UnknownKey { value, .. } => format!("unknown filter key {value:?}"),
            Self::EmptyValue => "empty value".into(),
            Self::UnsupportedComparison { key } => {
                format!("comparisons are unavailable for {key:?}")
            }
            Self::UnknownStatus { value } => format!("unknown status {value:?}"),
            Self::InvalidTag { value } => format!("invalid tag {value:?}"),
            Self::UnknownState { value } => format!("unknown state {value:?}"),
            Self::UnsupportedPresence { field } => format!("unsupported presence field {field:?}"),
            Self::InvalidDate { value } => format!("invalid date {value:?}"),
            Self::InvalidInteger { value } => format!("invalid integer {value:?}"),
            Self::DateOutOfRange { value } => {
                format!("date {value:?} is outside the supported calendar range")
            }
            Self::LowercaseKeyword { .. } => "keywords are lowercase".into(),
        }
    }

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

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason())?;
        if let Some(hint) = self.hint() {
            write!(f, ". {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ErrorKind {}

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
