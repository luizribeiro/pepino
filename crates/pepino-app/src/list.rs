use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    BoardId,
    filter::{Clause, Filter, Term},
};
use pepino_store::query::{self, Context, Query, QueryError, QueryErrorKind, Sort};

use crate::app::{invalid, store_error};
use crate::projection::project_summary;
use crate::state::storage;
use crate::{App, AppError, Date, FilterDiagnostic, FilterInput, MissingReference, Response};

type ViewFilters<'a> = BTreeMap<&'a str, (FilterInput, Filter)>;

impl App {
    pub(super) async fn list_tasks(
        &mut self,
        source: &FilterInput,
        sort: Option<&str>,
        limit: Option<usize>,
        today: Date,
    ) -> Result<Response, AppError> {
        let filter = source.parse(today)?;
        let sort: Sort = sort
            .unwrap_or("default")
            .parse()
            .map_err(|error| invalid("sort", error))?;
        self.reload_state().await?;
        let views = self
            .local
            .saved_views
            .iter()
            .map(|(name, text)| {
                let input = FilterInput::Text(text.into());
                let parsed = input
                    .parse(today)
                    .map_err(|error| storage("decode saved filter", error, false))?;
                Ok((name, (input, parsed)))
            })
            .collect::<Result<ViewFilters<'_>, AppError>>()?;
        let saved = |name: &str| views.get(name).map(|(_, filter)| filter.clone());
        let people = |name: &str| {
            (name == self.local.display_name)
                .then_some(self.local.identity_id)
                .into_iter()
                .collect()
        };
        let query = || Query {
            filter: &filter,
            context: Context {
                me: Some(self.local.identity_id),
                people: &people,
                views: &saved,
            },
        };
        let diagnostic = |error| query_error(error, source, &filter, &views);
        let terms = query().board_terms().map_err(diagnostic)?;
        let local_board = Board {
            id: self.local.board_id,
            name: &self.local.board_name,
        };
        let boards = select_boards(&[local_board], &terms)?;
        let (tasks, total) = if let Some(&board) = boards.first() {
            let view = if needs_comments(&filter, &views) {
                self.store.board_with_comments(board).await
            } else {
                self.store.board(board).await
            }
            .map_err(store_error)?;
            let mut matching = query::select(&view, &query()).map_err(diagnostic)?;
            let total = matching.len();
            sort.sort(&mut matching);
            let tasks = matching
                .into_iter()
                .take(limit.unwrap_or(total))
                .map(project_summary)
                .collect::<Result<_, _>>()?;
            (tasks, total)
        } else {
            (Vec::new(), 0)
        };
        Ok(Response::Tasks {
            tasks,
            total,
            filter: filter.to_string(),
            sort: sort.to_string(),
        })
    }
}

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

#[derive(Clone, Copy)]
struct Board<'a> {
    id: BoardId,
    name: &'a str,
}

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
