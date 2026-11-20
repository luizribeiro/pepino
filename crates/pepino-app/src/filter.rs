use std::{
    collections::BTreeMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use pepino_core::filter::{self, Clause, Filter};
use serde::{Deserialize, Serialize};

use crate::views::SavedViews;
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
            Self::Text(text) => filter::parse_at(text, today.into()),
            Self::Argv(argv) => filter::parse_argv_at(argv, today.into()),
        }
        .map_err(|error| AppError::InvalidInput {
            field: "filter".into(),
            reason: error.reason.into(),
            diagnostic: Some(Box::new(FilterDiagnostic {
                term: error.term.into(),
                position: Some(error.position.into()),
                view_chain: Vec::new(),
                hint: error.hint.map(String::from),
            })),
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
    pub(super) fn position_at(&self, index: usize) -> Option<FilterPosition> {
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

type ParsedViews = BTreeMap<String, (FilterInput, Result<Filter, AppError>)>;

pub(super) struct ViewFilters<'a> {
    saved: &'a SavedViews,
    today: Date,
    parsed: Mutex<ParsedViews>,
}

impl<'a> ViewFilters<'a> {
    pub(super) fn new(saved: &'a SavedViews, today: Date) -> Self {
        Self {
            saved,
            today,
            parsed: Mutex::new(BTreeMap::new()),
        }
    }

    pub(super) fn remember(&self, name: &str, source: FilterInput, filter: Filter) {
        self.parsed().insert(name.into(), (source, Ok(filter)));
    }

    pub(super) fn parsed(&self) -> MutexGuard<'_, ParsedViews> {
        self.parsed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn resolve(&self, name: &str) -> Option<Filter> {
        let raw = self.saved.get(name)?;
        self.parsed()
            .entry(name.into())
            .or_insert_with(|| {
                let input = FilterInput::Text(raw.into());
                let result = input.parse(self.today);
                (input, result)
            })
            .1
            .as_ref()
            .ok()
            .cloned()
    }

    pub(super) fn needs_comments(&self, filter: &Filter) -> bool {
        let has_text = |filter: &Filter| {
            filter
                .terms()
                .iter()
                .any(|term| matches!(&term.clause, Clause::Text(_)))
        };
        has_text(filter)
            || self
                .parsed()
                .values()
                .filter_map(|(_, parsed)| parsed.as_ref().ok())
                .any(has_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_filter_cache_only_parses_looked_up_views() {
        let today = "2026-11-20".parse().unwrap();
        let mut saved = SavedViews::default();
        saved.insert("weekend".into(), "tag:hardware".into());
        saved.insert("broken".into(), "tags:paint".into());
        let views = ViewFilters::new(&saved, today);
        assert!(views.resolve("weekend").is_some());
        assert!(!views.parsed().contains_key("broken"));
        assert!(views.resolve("broken").is_none());
        assert!(views.parsed()["broken"].1.is_err());
    }

    #[test]
    fn repeated_terms_have_separate_positions() {
        let text = FilterInput::Text("tag:hardware  tag:hardware".into());
        let argv = FilterInput::Argv(vec!["tag:hardware".into(), "  tag:hardware".into()]);
        assert_eq!(text.position_at(1), Some(FilterPosition::Byte { byte: 14 }));
        assert_eq!(
            argv.position_at(1),
            Some(FilterPosition::Argument { index: 1, byte: 2 })
        );
    }
}
