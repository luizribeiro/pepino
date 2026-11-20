#[path = "query/fixture.rs"]
mod fixture;
#[path = "query/semantics.rs"]
mod semantics;
#[path = "query/sort.rs"]
mod sort;

use pepino_core::{
    IdentityId,
    filter::{Filter, parse},
};
use pepino_store::query::{Context, Query, ViewResolver};

use fixture::date;

fn filter(input: &str) -> Filter {
    parse(input, date(19)).unwrap()
}

fn query(filter: &Filter) -> Query<'_> {
    Query {
        filter,
        context: Context {
            me: None,
            people: &people,
            views: &TestViews,
        },
    }
}

fn people(_: &str) -> Vec<IdentityId> {
    vec![]
}

struct TestViews;
impl ViewResolver for TestViews {
    fn resolve(&self, _: &str) -> Option<&Filter> {
        None
    }
}
