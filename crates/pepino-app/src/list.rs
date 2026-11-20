use crate::Date;
use crate::state::storage;
use crate::views::SavedViews;
use crate::{AppError, FilterDiagnostic, FilterInput, MissingReference};
use pepino_core::filter::Filter;
use pepino_store::query::{QueryError, QueryErrorKind};
use std::collections::{BTreeMap, BTreeSet};

type ViewFilters<'a> = BTreeMap<&'a str, (FilterInput, Filter)>;

use pepino_core::{
    BoardId,
    filter::{Clause, Term},
};

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Board<'a> {
    id: BoardId,
    name: &'a str,
}

#[allow(dead_code)]
fn select_boards(boards: &[Board<'_>], terms: &[Term]) -> Result<Vec<BoardId>, AppError> {
    let mut selected = boards.to_vec();
    for term in terms {
        let Clause::Board(names) = &term.clause else {
            continue;
        };
        let matches = |board: &Board<'_>| {
            names.iter().any(|name| {
                name == board.name || name.parse::<BoardId>().is_ok_and(|id| id == board.id)
            })
        };
        if !boards.iter().any(matches) {
            return Err(AppError::NotFound {
                field: "filter".into(),
                reference: MissingReference::Board(names.join(",")),
            });
        }
        selected.retain(|board| matches(board) != term.negated);
    }
    Ok(selected.into_iter().map(|board| board.id).collect())
}

#[allow(dead_code)]
fn query_error(
    error: QueryError,
    source: &FilterInput,
    filter: &Filter,
    views: &ViewFilters<'_>,
) -> AppError {
    let (source, filter) = error
        .view_chain
        .last()
        .and_then(|name| views.get(name.as_str()))
        .map_or((source, filter), |(source, filter)| (source, filter));
    let position = source.position_of(filter, &error.term);
    let reason = error.kind.to_string();
    let view_chain = match error.kind {
        QueryErrorKind::ViewCycle { chain } => chain,
        _ => error.view_chain,
    };
    AppError::InvalidInput {
        field: "filter".into(),
        reason,
        diagnostic: Some(Box::new(FilterDiagnostic {
            term: error.term,
            position,
            view_chain,
            hint: None,
        })),
    }
}

#[allow(dead_code)]
fn needs_comments<'a>(filter: &'a Filter, views: &'a ViewFilters<'_>) -> bool {
    let mut pending = vec![filter];
    let mut seen = BTreeSet::new();
    while let Some(filter) = pending.pop() {
        for term in filter.terms() {
            match &term.clause {
                Clause::Text(_) => return true,
                Clause::View(name) if seen.insert(name.as_str()) => {
                    if let Some((_, nested)) = views.get(name.as_str()) {
                        pending.push(nested);
                    }
                }
                _ => {}
            }
        }
    }
    false
}

#[allow(dead_code)]
fn parse_views(views: &SavedViews, today: Date) -> Result<ViewFilters<'_>, AppError> {
    views
        .iter()
        .map(|(name, text)| {
            let input = FilterInput::Text(text.into());
            let parsed = input
                .parse(today)
                .map_err(|error| storage("decode saved filter", error, false))?;
            Ok((name, (input, parsed)))
        })
        .collect()
}
