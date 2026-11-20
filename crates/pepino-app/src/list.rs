use std::collections::BTreeMap;

use pepino_core::{
    BoardId,
    filter::{Clause, Term},
};
use pepino_store::query::{Context, Query, QueryError, QueryErrorKind, Sort};

use crate::app::{invalid, store_error};
use crate::filter::ViewFilters;
use crate::projection::project_summary;
use crate::{App, AppError, Date, FilterDiagnostic, FilterInput, MissingReference, Response};

impl App {
    pub(super) async fn list_tasks(
        &mut self,
        source: &FilterInput,
        sort: Option<&str>,
        limit: Option<usize>,
        today: Date,
    ) -> Result<Response, AppError> {
        let filter = source.parse()?;
        let sort: Sort = sort
            .unwrap_or("default")
            .parse()
            .map_err(|error| invalid("sort", error))?;
        self.reload_state().await?;
        let views = ViewFilters::new(&self.local.saved_views);
        let saved = |name: &str| views.resolve(name);
        let people = |name: &str| {
            (name == self.local.display_name)
                .then_some(self.local.identity_id)
                .into_iter()
                .collect()
        };
        let query = || Query {
            filter: &filter,
            context: Context {
                today: today.into(),
                me: Some(self.local.identity_id),
                people: &people,
                views: &saved,
            },
        };
        let diagnostic = |error| query_error(error, source, &views, &self.local.display_name);
        let terms = query().board_terms().map_err(diagnostic)?;
        let local_board = Board {
            id: self.local.board_id,
            name: &self.local.board_name,
        };
        let boards = select_boards(&[local_board], &terms)?;
        let (tasks, total, resolved_dates, warnings) = if let Some(&board) = boards.first() {
            let view = if views.needs_comments(&filter) {
                self.store.board_with_comments(board).await
            } else {
                self.store.board(board).await
            }
            .map_err(store_error)?;
            let prepared = query().prepare(&view).map_err(diagnostic)?;
            let mut matching = prepared.select();
            let total = matching.len();
            sort.sort(&mut matching);
            let tasks = matching
                .into_iter()
                .take(limit.unwrap_or(total))
                .map(project_summary)
                .collect::<Result<Vec<_>, _>>()?;
            let resolved_dates = prepared
                .resolved_dates()
                .iter()
                .map(|(expression, date)| (expression.clone(), (*date).into()))
                .collect();
            let warnings = views.field_warnings(
                &filter,
                view.iter()
                    .flat_map(|task| task.custom.keys().map(|key| key.as_str())),
            );
            (tasks, total, resolved_dates, warnings)
        } else {
            (Vec::new(), 0, BTreeMap::new(), Vec::new())
        };
        let names = self.local.task_names(&tasks);
        Ok(Response::Tasks {
            names,
            warnings,
            tasks,
            total,
            filter: filter.to_string(),
            resolved_dates,
            sort: sort.to_string(),
        })
    }
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

pub(super) fn query_error(
    error: QueryError,
    source: &FilterInput,
    views: &ViewFilters<'_>,
    known_person: &str,
) -> AppError {
    let parsed = views.parsed();
    if let QueryErrorKind::UnknownView { name } = &error.kind
        && let Some((_, Err(error_from_view))) = parsed.get(name)
    {
        let mut error_from_view = error_from_view.clone();
        if let AppError::InvalidInput {
            diagnostic: Some(diagnostic),
            ..
        } = &mut error_from_view
        {
            diagnostic.view_chain = error.view_chain;
            diagnostic.view_chain.push(name.clone());
        }
        return error_from_view;
    }
    let source = error
        .view_chain
        .last()
        .and_then(|name| parsed.get(name.as_str()))
        .map_or(source, |(source, _)| source);
    let position = source.position_at(error.term_index);
    let reason = error.kind.to_string();
    let hint = match &error.kind {
        QueryErrorKind::UnknownView { .. } => {
            Some("Use list_views to see available saved views".into())
        }
        QueryErrorKind::UnknownPerson { .. } => Some(format!("Known person is {known_person}")),
        QueryErrorKind::AmbiguousPerson { candidates, .. } => Some(candidate_hint(candidates)),
        QueryErrorKind::AmbiguousTaskPrefix { candidates, .. } => Some(candidate_hint(candidates)),
        _ => None,
    };
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
            hint,
        })),
    }
}

fn candidate_hint(candidates: &[impl ToString]) -> String {
    let names = candidates
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    format!("Choose from {}", names.join(", "))
}
