use pepino_store::query::{matches, select};
use proptest::{prelude::*, sample};

use super::{filter, fixture::board, query};

const ALTERNATIVES: &[(&str, &[&str])] = &[
    ("tag", &["garden", "hardware", "paint", "nonexistent"]),
    ("status", &["todo", "doing", "done", "cancelled"]),
    ("assignee", &["me", "bob", r#""Ana Maria""#]),
    ("parent", &["abcd01", "0808", "0404"]),
    ("blocks", &["abcd01", "0707", "abcd02"]),
    ("blocked-by", &["abcd01", "0303", "abcd02"]),
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
    ("due", &["<2026-11-20", "2026-11-20", ">2026-11-20"]),
    ("created", &["<2026-11-19", "2026-11-19", ">=2026-11-20"]),
    ("updated", &["<2026-11-20", "2026-11-20", ">2026-11-20"]),
    ("estimate", &["<10", "12", r#""12""#, "2026-11-20"]),
    ("inspection", &["<2026-11-21", "2026-11-21", ">2026-11-21"]),
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
            "@local"
        ])
        .prop_map(str::to_owned),
    ]
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
}
