use pepino_core::filter::{self, Filter};
use serde::{Deserialize, Serialize};

use crate::{AppError, Date};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum FilterInput {
    Text(String),
    Argv(Vec<String>),
}

impl FilterInput {
    pub(super) fn parse(&self, today: Date) -> Result<Filter, AppError> {
        match self {
            Self::Text(text) => filter::parse(text, today.into()),
            Self::Argv(argv) => filter::parse_argv(argv, today.into()),
        }
        .map_err(|error| {
            let message = error.kind.to_string();
            let (reason, hint) = message
                .rsplit_once(". ")
                .map_or((message.as_str(), None), |(reason, hint)| {
                    (reason, Some(hint.into()))
                });
            AppError::InvalidInput {
                field: "filter".into(),
                reason: reason.into(),
                diagnostic: Some(Box::new(FilterDiagnostic {
                    term: error.term,
                    position: error.position.into(),
                    view_chain: Vec::new(),
                    hint,
                })),
            }
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilterDiagnostic {
    pub term: String,
    pub position: FilterPosition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub view_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum FilterPosition {
    Byte { byte: usize },
    Argument { index: usize, byte: usize },
}

impl From<filter::Position> for FilterPosition {
    fn from(position: filter::Position) -> Self {
        match position {
            filter::Position::Byte(byte) => Self::Byte { byte },
            filter::Position::Argument { index, byte } => Self::Argument { index, byte },
        }
    }
}
