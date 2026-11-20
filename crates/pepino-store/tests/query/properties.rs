use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    TaskId,
    filter::{Filter, Term},
};
use pepino_store::query::{QueryError, matches, select};
use proptest::{prelude::*, sample};

use super::{filter, fixture::board, query, views};

const ALTERNATIVES: &[(&str, &[&str])] = &[
    ("tag", &["garden", "hardware", "paint", "nonexistent"]),
    ("status", &["todo", "doing", "done", "cancelled"]),
    ("assignee", &["me", "bob", r#""Ana Maria""#]),
    ("parent", &["abcd87", "31d6", "b3dc"]),
    ("blocks", &["abcd87", "949b", "abcd9b"]),
    ("blocked-by", &["abcd87", "aa64", "abcd9b"]),
    ("is", &["blocked", "ready", "open", "closed"]),
    (
        "has",
        &[
            "description",
            "parent",
            "due",
            "estimate",
            "inspection",
            "budget",
        ],
    ),
    ("priority", &["<0", "5", ">10"]),
    ("due", &["<fri", "2026-11-20", ">tomorrow"]),
    ("created", &["<today", "2026-11-19", ">=tomorrow"]),
    ("updated", &["<2026-11-20", "tomorrow", ">+1w"]),
    ("estimate", &["<10", "12", r#""12""#, "2026-11-20"]),
    ("inspection", &["<2026-11-21", "+2d", ">=-1w"]),
    ("supplier", &[r#"<"Beta""#, r#""Acme""#, "12"]),
];

fn alternatives() -> impl Strategy<Value = (&'static str, Vec<&'static str>)> {
    sample::select(ALTERNATIVES)
        .prop_flat_map(|(key, values)| (Just(key), sample::subsequence(values, 1..=values.len())))
}

fn term() -> impl Strategy<Value = String> {
    prop_oneof![
        alternatives().prop_map(|(key, values)| format!("{key}:{}", values.join(","))),
        sample::select(vec![
            "fence",
            "STRASSE",
            "οδυσσεύσ",
            "bolts",
            "@weekend",
            "@empty",
            "@garden-open"
        ])
        .prop_map(str::to_owned),
    ]
}

fn ids(input: &str) -> BTreeSet<TaskId> {
    select(board(), &query(&filter(input)))
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect()
}

proptest! {
    #[test]
    fn select_equals_filtering_tasks_with_matches(
        terms in prop::collection::vec((any::<bool>(), term()), 0..8)
    ) {
        let input = terms
            .into_iter()
            .map(|(negated, term)| if negated { format!("-{term}") } else { term })
            .collect::<Vec<_>>()
            .join(" ");
        let filter = filter(&input);
        let query = query(&filter);
        let view = board();
        let filtered: Vec<_> = view
            .tasks()
            .iter()
            .filter(|task| matches(view, task, &query).unwrap())
            .collect();
        prop_assert_eq!(select(view, &query).unwrap(), filtered);
    }

    #[test]
    fn a_term_and_its_negation_partition_the_board(term in term()) {
        let positive = ids(&term);
        let negative = ids(&format!("-{term}"));
        prop_assert!(positive.is_disjoint(&negative));
        let union = positive.union(&negative).copied().collect::<BTreeSet<_>>();
        prop_assert_eq!(union, ids(""));
    }

    #[test]
    fn conjunction_is_intersection(left in term(), right in term(), negate_left in any::<bool>()) {
        let left = if negate_left { format!("-{left}") } else { left };
        let intersection = ids(&left).intersection(&ids(&right)).copied().collect::<BTreeSet<_>>();
        prop_assert_eq!(ids(&format!("{left} {right}")), intersection);
    }

    #[test]
    fn comma_alternatives_are_union((key, alternatives) in alternatives()) {
        let union = alternatives
            .iter()
            .flat_map(|value| ids(&format!("{key}:{value}")))
            .collect::<BTreeSet<_>>();
        prop_assert_eq!(ids(&format!("{key}:{}", alternatives.join(","))), union);
    }
}

fn error_term() -> impl Strategy<Value = &'static str> {
    sample::select(vec![
        "assignee:missing",
        "updated:+9223372036854775807w",
        "inspection:-99999999999999999999999d",
        "assignee:Alex",
        "parent:xy",
        "blocks:0f0f",
        "blocked-by:abcd",
        "@missing",
    ])
}

fn generated_term() -> impl Strategy<Value = String> {
    (
        any::<bool>(),
        prop_oneof![
            term(),
            error_term().prop_map(str::to_owned),
            sample::select(vec!["@v0", "@v1", "@v2", "assignee:me", "board:garden"])
                .prop_map(str::to_owned),
        ],
    )
        .prop_map(|(negated, term)| if negated { format!("-{term}") } else { term })
}

fn assert_permutations(
    terms: &mut [Term],
    index: usize,
    saved: &BTreeMap<&str, Filter>,
    me: bool,
    expected: &QueryError,
) {
    if index == terms.len() {
        let filter = Filter::from_terms(terms.to_vec());
        let mut query = query(&filter);
        query.context.views = saved;
        if !me {
            query.context.me = None;
        }
        let view = board();
        let mut expected = expected.clone();
        let source = expected
            .view_chain
            .last()
            .map_or(&filter, |name| &saved[name.as_str()]);
        expected.term_index = source
            .terms()
            .iter()
            .position(|term| term.to_string() == expected.term)
            .unwrap();
        assert_eq!(select(view, &query).unwrap_err(), expected);
        assert_eq!(
            matches(view, &view.tasks()[0], &query).unwrap_err(),
            expected
        );
        return;
    }
    for swap in index..terms.len() {
        terms.swap(index, swap);
        assert_permutations(terms, index + 1, saved, me, expected);
        terms.swap(index, swap);
    }
}

proptest! {
    #[test]
    fn every_term_permutation_returns_the_same_error(
        valid in term(),
        failing in error_term(),
        rest in prop::collection::vec(generated_term(), 0..=2),
        definitions in prop::collection::vec(prop::collection::vec(generated_term(), 0..=3), 3),
    ) {
        let mut saved = views().clone();
        for (name, definition) in ["v0", "v1", "v2"].into_iter().zip(definitions) {
            saved.insert(name, filter(&definition.join(" ")));
        }
        let mut inputs = vec![valid, failing.to_owned(), "@v0".into()];
        inputs.extend(rest);
        let root = filter(&inputs.join(" "));
        let mut terms = root.terms().to_vec();
        for me in [true, false] {
            let mut query = query(&root);
            query.context.views = &saved;
            if !me {
                query.context.me = None;
            }
            let expected = select(board(), &query).unwrap_err();
            assert_permutations(&mut terms, 0, &saved, me, &expected);
        }
    }
}
