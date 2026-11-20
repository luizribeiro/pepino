use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, MutexGuard, PoisonError},
};

use pepino_core::filter::{self, Clause, CustomValue, Filter, PresenceField, State};
use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::views::SavedViews;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum FilterInput {
    Text(String),
    Argv(Vec<String>),
}

impl FilterInput {
    /// Combines a filter string and split arguments, returning the byte offset of each argv term.
    pub fn combine(mut text: String, argv: Vec<String>) -> Result<(Self, Vec<usize>), AppError> {
        if argv.is_empty() {
            return Ok((Self::Text(text), vec![]));
        }
        Self::Text(text.clone()).parse()?;
        let terms = Self::Argv(argv).parse()?;
        let mut starts = Vec::with_capacity(terms.terms().len());
        for term in terms.terms() {
            text.push(' ');
            starts.push(text.len());
            text.push_str(&term.to_string());
        }
        Ok((Self::Text(text), starts))
    }

    /// Checks whether the input can be parsed without resolving references.
    pub fn syntax_is_valid(&self) -> bool {
        self.parse().is_ok()
    }

    /// Checks whether the supplied filter explicitly selects a status or state.
    pub fn has_status_term(&self) -> Result<bool, AppError> {
        Ok(self
            .parse()?
            .terms()
            .iter()
            .any(|term| matches!(term.clause, Clause::Status(_) | Clause::Is(_))))
    }

    pub(super) fn parse(&self) -> Result<Filter, AppError> {
        match self {
            Self::Text(text) => filter::parse(text),
            Self::Argv(argv) => filter::parse_argv(argv),
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilterWarning {
    pub field: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suggestion: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_tasks: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const BUILTIN_KEYS: &[&str] = &[
    "assignee",
    "blocked-by",
    "blocks",
    "board",
    "created",
    "due",
    "has",
    "is",
    "parent",
    "priority",
    "status",
    "tag",
    "updated",
];

type ParsedViews = BTreeMap<String, (FilterInput, Result<Filter, AppError>)>;

pub(super) struct ViewFilters<'a> {
    saved: &'a SavedViews,
    parsed: Mutex<ParsedViews>,
}

impl<'a> ViewFilters<'a> {
    pub(super) fn new(saved: &'a SavedViews) -> Self {
        Self {
            saved,
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
                let result = input.parse();
                (input, result)
            })
            .1
            .as_ref()
            .ok()
            .cloned()
    }

    pub(super) fn field_warnings<'b>(
        &self,
        filter: &Filter,
        fields: impl Iterator<Item = (&'b str, bool)>,
    ) -> Vec<FilterWarning> {
        let mut text_counts = BTreeMap::new();
        for (field, text) in fields {
            *text_counts.entry(field).or_insert(0usize) += usize::from(text);
        }
        let parsed = self.parsed();
        let mut referenced = BTreeSet::new();
        let mut numeric = BTreeSet::new();
        for term in std::iter::once(filter)
            .chain(
                parsed
                    .values()
                    .filter_map(|(_, parsed)| parsed.as_ref().ok()),
            )
            .flat_map(Filter::terms)
        {
            match &term.clause {
                Clause::Custom {
                    field,
                    alternatives,
                } => {
                    referenced.insert(field.as_str());
                    if alternatives
                        .iter()
                        .any(|item| matches!(item.value, CustomValue::Integer(_)))
                    {
                        numeric.insert(field.as_str());
                    }
                }
                Clause::Has(alternatives) => {
                    referenced.extend(alternatives.iter().filter_map(|field| match field {
                        PresenceField::Custom(name) => Some(name.as_str()),
                        _ => None,
                    }))
                }
                _ => {}
            }
        }
        referenced.into_iter().filter_map(|field| {
                if let Some(&count) = text_counts.get(field) {
                    return (count > 0 && numeric.contains(field)).then(|| FilterWarning {
                        field: field.into(),
                        suggestion: String::new(),
                        text_tasks: Some(count),
                        message: Some(format!("field {field} holds text on {count} tasks; numeric comparison never matches them")),
                    });
                }
                let suggestion = BUILTIN_KEYS
                    .iter()
                    .copied()
                    .chain(text_counts.keys().copied())
                    .min_by_key(|candidate| {
                        (strsim::damerau_levenshtein(field, candidate), *candidate)
                    })?;
                Some(FilterWarning {
                    field: field.into(),
                    suggestion: suggestion.into(),
                    text_tasks: None,
                    message: None,
                })
            })
            .collect()
    }

    pub(super) fn constrains_status(&self, filter: &Filter) -> bool {
        let parsed = self.parsed();
        std::iter::once(filter)
            .chain(
                parsed
                    .values()
                    .filter_map(|(_, parsed)| parsed.as_ref().ok()),
            )
            .flat_map(Filter::terms)
            .any(|term| match &term.clause {
                Clause::Status(_) => true,
                Clause::Is(states) => states
                    .iter()
                    .any(|state| matches!(state, State::Open | State::Closed)),
                _ => false,
            })
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
    fn combining_filters_preserves_negated_phrases_and_positional_values() {
        let (input, starts) = FilterInput::combine(
            r#" tag:hardware  -"bad hinge""#.into(),
            vec!["assignee:Ana Maria".into(), "fix the fence".into()],
        )
        .unwrap();
        assert_eq!(
            input.position_at(2),
            Some(FilterPosition::Byte { byte: starts[0] })
        );
        assert_eq!(
            input.position_at(3),
            Some(FilterPosition::Byte { byte: starts[1] })
        );
        assert_eq!(
            input.parse().unwrap().to_string(),
            r#"tag:hardware -"bad hinge" assignee:"Ana Maria" "fix the fence""#
        );
    }

    #[test]
    fn status_detection_uses_the_filter_grammar() {
        for (arg, expected) in [
            ("  status:done", true),
            ("-is:blocked", true),
            ("\"status:done\"", false),
            ("tag:hardware", false),
            ("@weekend", false),
        ] {
            assert_eq!(
                FilterInput::Argv(vec![arg.into()])
                    .has_status_term()
                    .unwrap(),
                expected,
                "{arg}"
            );
        }
        assert!(
            FilterInput::Argv(vec!["status:finished".into()])
                .has_status_term()
                .is_err()
        );
    }

    #[test]
    fn saved_filter_cache_only_parses_looked_up_views() {
        let mut saved = SavedViews::default();
        saved.insert("weekend".into(), "tag:hardware".into());
        saved.insert("broken".into(), "tags:paint".into());
        let views = ViewFilters::new(&saved);
        assert!(views.resolve("weekend").is_some());
        assert!(!views.parsed().contains_key("broken"));
        assert!(views.resolve("broken").is_none());
        assert!(views.parsed()["broken"].1.is_err());
    }

    #[test]
    fn unknown_fields_include_reachable_views_and_prefer_the_closest_key() {
        let mut saved = SavedViews::default();
        saved.insert(
            "weekend".into(),
            "has:estmate estimate:3 statuss:todo".into(),
        );
        let views = ViewFilters::new(&saved);
        assert!(views.resolve("weekend").is_some());
        let filter = filter::parse("statuss:doing @weekend").unwrap();
        assert_eq!(
            views.field_warnings(&filter, [("estimate", false)].into_iter()),
            [
                FilterWarning {
                    field: "estmate".into(),
                    suggestion: "estimate".into(),
                    text_tasks: None,
                    message: None,
                },
                FilterWarning {
                    field: "statuss".into(),
                    suggestion: "status".into(),
                    text_tasks: None,
                    message: None,
                },
            ]
        );
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
