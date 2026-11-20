#[path = "query/errors.rs"]
mod errors;
#[path = "query/fixture.rs"]
mod fixture;
#[path = "query/semantics.rs"]
mod semantics;
#[path = "query/sort.rs"]
mod sort;

use std::{collections::BTreeMap, sync::OnceLock};

use pepino_core::{
    IdentityId,
    filter::{Filter, parse},
};
use pepino_store::query::{Context, Query, ViewResolver};

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
            views: &TestViews,
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

struct TestViews;

impl ViewResolver for TestViews {
    fn resolve(&self, name: &str) -> Option<&Filter> {
        static VIEWS: OnceLock<BTreeMap<&str, Filter>> = OnceLock::new();
        VIEWS
            .get_or_init(|| {
                [
                    ("garden-open", "tag:garden is:open"),
                    ("weekend", "@garden-open due:2026-11-20"),
                    ("empty", ""),
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
            .get(name)
    }
}
