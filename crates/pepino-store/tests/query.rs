#[path = "query/errors.rs"]
mod errors;
#[path = "query/fixture.rs"]
mod fixture;
#[path = "query/properties.rs"]
mod properties;
#[path = "query/semantics.rs"]
mod semantics;
#[path = "query/sort.rs"]
mod sort;

use std::{collections::BTreeMap, sync::OnceLock};

use pepino_core::{
    IdentityId,
    filter::{Filter, parse},
};
use pepino_store::query::{Context, Query};

use fixture::{ANA, BOB, date};

fn filter(input: &str) -> Filter {
    parse(input, date(19)).unwrap()
}

fn query(filter: &Filter) -> Query<'_> {
    Query {
        filter,
        context: Context {
            me: Some(ANA),
            people: &people,
            views: views(),
        },
    }
}

fn people(text: &str) -> Vec<IdentityId> {
    match text {
        "Ana Maria" | "ana" => vec![ANA],
        "bob" | "me" => vec![BOB],
        "Alex" => vec![BOB, ANA],
        "ana-work" => vec![ANA, ANA],
        _ => vec![],
    }
}

fn views() -> &'static BTreeMap<&'static str, Filter> {
    static VIEWS: OnceLock<BTreeMap<&str, Filter>> = OnceLock::new();
    VIEWS.get_or_init(|| {
        [
            ("garden-open", "tag:garden is:open"),
            ("weekend", "@garden-open due:2026-11-20"),
            ("empty", ""),
            ("fence-details", "fence cèdre"),
            ("local", "-board:elsewhere tag:garden"),
            ("self", "@self"),
            ("entry", "@cycle-a"),
            ("cycle-a", "@cycle-b"),
            ("cycle-b", "-@cycle-a"),
            ("unknown", "@missing"),
        ]
        .into_iter()
        .map(|(name, input)| (name, filter(input)))
        .collect()
    })
}
