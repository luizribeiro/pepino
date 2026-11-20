use pepino_store::query::{Query, QueryError, QueryErrorKind, matches, select};

use super::{
    filter,
    fixture::{board, id},
    query,
};

fn check(query: &Query<'_>, term: &str, kind: QueryErrorKind) {
    let expected = QueryError {
        term: term.into(),
        kind,
    };
    assert_eq!(select(board(), query).unwrap_err(), expected);
    let task = board().task(id(0)).unwrap();
    let error = matches(board(), task, query).unwrap_err();
    assert_eq!(error, expected);
    assert!(error.to_string().starts_with(term));
}

#[test]
fn invalid_task_prefix_is_a_typed_error() {
    check(
        &query(&filter("parent:xy")),
        "parent:xy",
        QueryErrorKind::InvalidTaskPrefix {
            prefix: "xy".into(),
        },
    );
}

#[test]
fn deleted_task_prefix_resolves_as_unknown() {
    check(
        &query(&filter("blocks:0f0f")),
        "blocks:0f0f",
        QueryErrorKind::UnknownTaskPrefix {
            prefix: "0f0f".into(),
        },
    );
}

#[test]
fn ambiguous_task_prefix_lists_candidates_and_total() {
    check(
        &query(&filter("blocked-by:abcd")),
        "blocked-by:abcd",
        QueryErrorKind::AmbiguousTaskPrefix {
            prefix: "abcd".into(),
            count: 2,
            candidates: vec![id(0), id(1)],
        },
    );
}
