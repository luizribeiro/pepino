use pepino_core::filter;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum FilterInput {
    Text(String),
    Argv(Vec<String>),
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
