use crate::{AppError, FilterDiagnostic, FilterInput, MissingReference};
use pepino_core::filter::Filter;
use pepino_store::query::{QueryError, QueryErrorKind};
use std::collections::BTreeMap;

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
