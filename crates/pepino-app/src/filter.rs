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
                    position: Some(error.position.into()),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<FilterPosition>,
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

impl FilterInput {
    pub(super) fn position_of(&self, filter: &Filter, term: &str) -> Option<FilterPosition> {
        let index = filter
            .terms()
            .iter()
            .position(|parsed| parsed.to_string() == term)?;
        match self {
            Self::Text(text) => term_starts(text)
                .nth(index)
                .map(|byte| FilterPosition::Byte { byte }),
            Self::Argv(argv) => argv.get(index).map(|arg| FilterPosition::Argument {
                index,
                byte: arg.len() - arg.trim_start().len(),
            }),
        }
    }
}

fn term_starts(text: &str) -> impl Iterator<Item = usize> {
    let mut quoted = false;
    let mut escaped = false;
    let mut in_term = false;
    text.char_indices().filter_map(move |(index, ch)| {
        if !quoted && ch.is_whitespace() {
            in_term = false;
            return None;
        }
        if escaped {
            escaped = false;
        } else if ch == '"' {
            quoted = !quoted;
        } else if quoted && ch == '\\' {
            escaped = true;
        }
        let start = !in_term;
        in_term = true;
        start.then_some(index)
    })
}
