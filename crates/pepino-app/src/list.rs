use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    IdentityId,
    filter::{Clause, Filter, State, Term},
};
use pepino_store::BoardView;
use pepino_store::query::{Context, Query, QueryError, QueryErrorKind, Sort, fold_text};

use crate::app::{invalid, store_error};
use crate::filter::ViewFilters;
use crate::projection::project_summary;
use crate::{
    App, AppError, Date, FilterDiagnostic, FilterInput, LocalState, MissingReference, Response,
};

impl App {
    pub(super) async fn list_tasks(
        &mut self,
        source: &FilterInput,
        sort: Option<&str>,
        limit: Option<usize>,
        today: Date,
        open_default: bool,
    ) -> Result<Response, AppError> {
        let supplied = source.parse()?;
        let sort: Sort = sort
            .unwrap_or("default")
            .parse()
            .map_err(|error| invalid("sort", error))?;
        let views = ViewFilters::new(&self.local.saved_views);
        let saved = |name: &str| views.resolve(name);
        let people = |name: &str| self.local.person_candidates(None, name);
        let context = || Context {
            today: today.into(),
            me: Some(self.local.identity_id),
            people: &people,
            views: &saved,
        };
        let diagnostic = |error| query_error(error, source, &views, &self.local.display_name);
        let terms = (Query {
            filter: &supplied,
            context: context(),
        })
        .board_terms()
        .map_err(diagnostic)?;
        let constrains_status = views.constrains_status(&supplied);
        let filter = if open_default && !constrains_status {
            let open = Term {
                negated: false,
                clause: Clause::Is(vec![State::Open]),
            };
            Filter::from_terms(supplied.terms().iter().cloned().chain([open]).collect())
        } else {
            supplied
        };
        let boards = self.list_boards_for_filter(&terms)?;
        let include_board = self.all_boards || boards.len() > 1;
        let mut snapshots = Vec::new();
        for board in boards {
            let view = if views.needs_comments(&filter) {
                self.store.board_with_comments(board.id).await
            } else {
                self.store.board(board.id).await
            }
            .map_err(store_error)?;
            snapshots.push((board, view));
        }
        let people = |name: &str| {
            snapshots
                .iter()
                .flat_map(|(_, view)| self.local.person_candidates(Some(view), name))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        let mut matching = Vec::new();
        let mut resolved_dates = BTreeMap::new();
        for (board, view) in &snapshots {
            let prepared = (Query {
                filter: &filter,
                context: Context {
                    people: &people,
                    ..context()
                },
            })
            .prepare(view)
            .map_err(diagnostic)?;
            matching.extend(prepared.select().into_iter().map(|task| (*board, task)));
            resolved_dates.extend(
                prepared
                    .resolved_dates()
                    .iter()
                    .map(|(expr, date)| (expr.clone(), (*date).into())),
            );
        }
        let total = matching.len();
        matching.sort_unstable_by(|(left_board, left), (right_board, right)| {
            sort.compare(left, right)
                .then_with(|| left_board.id.cmp(&right_board.id))
        });
        let tasks = matching
            .into_iter()
            .take(limit.unwrap_or(total))
            .map(|(board, task)| {
                let mut task = project_summary(task)?;
                task.board = include_board.then(|| board.clone());
                Ok(task)
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let warnings = if snapshots.is_empty() {
            Vec::new()
        } else {
            views.field_warnings(
                &filter,
                snapshots
                    .iter()
                    .flat_map(|(_, view)| view.iter())
                    .flat_map(|task| {
                        task.custom.iter().map(|(key, value)| {
                            (
                                key.as_str(),
                                matches!(value, pepino_core::FieldValue::Text(_)),
                            )
                        })
                    }),
            )
        };
        let names = self.local.task_names(&tasks);
        Ok(Response::Tasks {
            names,
            warnings,
            constrains_status,
            tasks,
            total,
            filter: filter.to_string(),
            resolved_dates,
            sort: sort.to_string(),
        })
    }
}

impl LocalState {
    pub(super) fn person_candidates(
        &self,
        view: Option<&BoardView>,
        name: &str,
    ) -> Vec<IdentityId> {
        if let Ok(id) = name.parse() {
            return vec![id];
        }
        let named = (fold_text(name) == fold_text(&self.display_name)).then_some(self.identity_id);
        if name.len() < 4 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return named.into_iter().collect();
        }
        let prefix = name.to_ascii_lowercase();
        named
            .into_iter()
            .chain(
                view.into_iter()
                    .flat_map(BoardView::assignee_ids)
                    .copied()
                    .chain([self.identity_id])
                    .filter(|id| id.to_string().starts_with(&prefix)),
            )
            .collect()
    }
}

impl App {
    fn list_boards_for_filter(&self, terms: &[Term]) -> Result<Vec<&crate::LocalBoard>, AppError> {
        let mut selected = if self.all_boards || (!terms.is_empty() && self.selected.is_none()) {
            self.local.boards().iter().collect()
        } else {
            vec![self.selected_board()]
        };
        for term in terms {
            let Clause::Board(names) = &term.clause else {
                continue;
            };
            let mut matches = BTreeSet::new();
            for name in names {
                match self.local.resolve_board(name, "filter") {
                    Ok(board) => {
                        matches.insert(board.id);
                    }
                    Err(AppError::NotFound { .. }) => (),
                    Err(error) => return Err(error),
                }
            }
            if matches.is_empty() {
                return Err(AppError::NotFound {
                    field: "filter".into(),
                    reference: MissingReference::Board(names.join(",")),
                });
            }
            selected.retain(|board| matches.contains(&board.id) != term.negated);
        }
        Ok(selected)
    }
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
        _ => None,
    };
    let mut diagnostic = FilterDiagnostic {
        term: error.term,
        position,
        view_chain: error.view_chain,
        hint,
    };
    if let QueryErrorKind::AmbiguousTaskPrefix {
        prefix,
        count,
        candidates,
    } = error.kind
    {
        return AppError::AmbiguousId {
            field: "filter".into(),
            task: prefix,
            count,
            candidates: candidates.iter().map(ToString::to_string).collect(),
            diagnostic: Some(Box::new(diagnostic)),
        };
    }
    if let QueryErrorKind::ViewCycle { chain } = error.kind {
        diagnostic.view_chain = chain;
    }
    AppError::InvalidInput {
        field: "filter".into(),
        reason,
        diagnostic: Some(Box::new(diagnostic)),
    }
}

fn candidate_hint(candidates: &[impl ToString]) -> String {
    let names = candidates
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    format!("Choose from {}", names.join(", "))
}
#[cfg(test)]
#[tokio::test]
async fn routes_a_filter_to_a_board_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let id = pepino_core::BoardId::from_bytes([81; 32]);
    app.local.boards.push(crate::LocalBoard {
        id,
        name: "Workshop".into(),
    });
    let filter = FilterInput::Text("board:5151".into()).parse().unwrap();
    let boards = app.list_boards_for_filter(filter.terms()).unwrap();
    assert_eq!(boards[0].id, id);
}
