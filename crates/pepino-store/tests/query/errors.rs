use pepino_core::BoardId;
use pepino_store::{
    Store,
    query::{Query, QueryError, QueryErrorKind, matches, select},
};

use super::{
    ANA, BOB, filter,
    fixture::{board, id},
    query,
};

fn check(query: &Query<'_>, term: &str, kind: QueryErrorKind) {
    check_chain(query, term, kind, &[]);
}

fn check_chain(query: &Query<'_>, term: &str, kind: QueryErrorKind, chain: &[&str]) {
    let expected = QueryError {
        term: term.into(),
        kind,
        view_chain: chain.iter().map(|name| (*name).to_owned()).collect(),
    };
    assert_eq!(query.prepare(board()).unwrap_err(), expected);
    assert_eq!(select(board(), query).unwrap_err(), expected);
    let task = board().task(id(0)).unwrap();
    let error = matches(board(), task, query).unwrap_err();
    assert_eq!(error, expected);
    if !chain.is_empty() {
        assert!(
            error
                .to_string()
                .starts_with(&format!("in @{}: {term}", chain.join(" -> @")))
        );
    }
}

#[test]
fn me_without_a_current_identity_fails_even_in_a_negated_term() {
    let filter = filter("-assignee:ana,me");
    let mut query = query(&filter);
    query.context.me = None;
    check(&query, "-assignee:ana,me", QueryErrorKind::MissingIdentity);
}

#[test]
fn unresolved_person_reports_decoded_text_and_canonical_term() {
    let term = r#"-assignee:ana,"Missing Person""#;
    let filter = filter(&format!(r#"tag:absent {term}"#));
    let kind = QueryErrorKind::UnknownPerson {
        text: "Missing Person".into(),
    };
    check(&query(&filter), term, kind.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let empty = runtime.block_on(async {
        Store::open_in_memory()
            .await
            .unwrap()
            .board(BoardId::from_bytes([0; 32]))
            .await
            .unwrap()
    });
    assert_eq!(
        select(&empty, &query(&filter)),
        Err(QueryError {
            term: term.into(),
            kind,
            view_chain: vec![],
        })
    );
}

#[test]
fn ambiguous_person_lists_candidate_identities_in_id_order() {
    check(
        &query(&filter("assignee:Alex")),
        "assignee:Alex",
        QueryErrorKind::AmbiguousPerson {
            text: "Alex".into(),
            candidates: vec![ANA, BOB],
        },
    );
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

#[test]
fn unknown_saved_view_names_the_failing_reference() {
    check_chain(
        &query(&filter("-@unknown")),
        "@missing",
        QueryErrorKind::UnknownView {
            name: "missing".into(),
        },
        &["unknown"],
    );
}

#[test]
fn direct_and_indirect_view_cycles_report_the_chain_and_closing_term() {
    for (input, term, chain) in [
        ("@self", "@self", vec!["self", "self"]),
        (
            "@entry",
            "-@cycle-a",
            vec!["entry", "cycle-a", "cycle-b", "cycle-a"],
        ),
    ] {
        let kind = QueryErrorKind::ViewCycle {
            chain: chain.iter().map(|name| (*name).to_owned()).collect(),
        };
        check_chain(
            &query(&filter(input)),
            term,
            kind,
            &chain[..chain.len() - 1],
        );
    }
}

#[test]
fn preparation_errors_are_independent_of_term_order() {
    for input in [
        "parent:xy tag:garden assignee:missing",
        "tag:garden assignee:missing parent:xy",
        "assignee:missing parent:xy tag:garden",
    ] {
        check(
            &query(&filter(input)),
            "assignee:missing",
            QueryErrorKind::UnknownPerson {
                text: "missing".into(),
            },
        );
    }
}

#[test]
fn nested_clause_errors_report_the_views_that_led_there() {
    let root = filter("@weekend");
    let views = std::collections::BTreeMap::from([
        ("weekend", filter("@garden")),
        ("garden", filter("assignee:missing")),
    ]);
    let mut query = query(&root);
    query.context.views = &views;
    check_chain(
        &query,
        "assignee:missing",
        QueryErrorKind::UnknownPerson {
            text: "missing".into(),
        },
        &["weekend", "garden"],
    );
}

#[tokio::test]
async fn text_search_requires_comments_even_for_an_empty_board() {
    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([0; 32]);
    let plain = store.board(board).await.unwrap();
    assert!(
        select(&plain, &query(&filter("tag:garden")))
            .unwrap()
            .is_empty()
    );
    let text = filter("@woodwork");
    let saved = std::collections::BTreeMap::from([("woodwork", filter("-fence"))]);
    let mut query = query(&text);
    query.context.views = &saved;
    let error = select(&plain, &query).unwrap_err();
    assert_eq!(error.kind, QueryErrorKind::CommentsRequired);
    assert_eq!(error.term, "-fence");
    assert_eq!(error.view_chain, ["woodwork"]);
    let with_comments = store.board_with_comments(board).await.unwrap();
    assert!(select(&with_comments, &query).unwrap().is_empty());
}
