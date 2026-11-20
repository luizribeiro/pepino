use pepino_store::query::select;

use super::{
    filter,
    fixture::{board, id},
    query,
};

fn check(input: &str, expected: &[u8]) {
    let filter = filter(input);
    let query = query(&filter);
    let mut actual: Vec<_> = select(board(), &query)
        .unwrap()
        .into_iter()
        .map(|task| task.id)
        .collect();
    let mut expected: Vec<_> = expected.iter().copied().map(id).collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "{input}");
}

#[test]
fn terms_conjoin_alternatives_disjoin_and_negation_applies_to_the_whole_term() {
    check("tag:garden,hardware status:todo -tag:hardware", &[11]);
    check("tag:garden -status:doing,done", &[0, 11]);
    check("tag:garden tag:hardware", &[0]);
    check("status:doing,cancelled", &[1, 3, 7]);
    assert_eq!(
        select(board(), &query(&filter(""))).unwrap(),
        board().iter().collect::<Vec<_>>()
    );
}

#[test]
fn compares_priority_numerically_and_due_chronologically() {
    check("priority:<0", &[4]);
    check("priority:>5", &[2]);
    check("priority:<=5", &[0, 1, 4, 7, 8, 10, 11]);
    check("priority:5", &[0, 1, 7, 8, 11]);
    check("-priority:>=0", &[3, 4, 5, 6, 9, 12, 13]);
    check("due:2026-11-20", &[0, 6, 7, 11]);
    check("due:>=2026-11-20", &[0, 6, 7, 10, 11]);
    check("due:<2026-11-20", &[1, 2]);
    check("-due:<=2026-11-20", &[3, 4, 5, 8, 9, 10, 12, 13]);
}

#[test]
fn creation_and_updates_compare_by_utc_wall_date_at_midnight() {
    check("created:2026-11-19", &[0, 7]);
    check("created:2026-11-20", &[11]);
    check("tag:garden updated:2026-11-20", &[0, 2, 7, 11]);
    check("tag:garden updated:<2026-11-20", &[]);
    check("created:>2026-11-20 updated:>2026-11-20", &[13]);
    check("created:<1970-01-01", &[]);
}

#[test]
fn state_predicates_use_status_and_effective_blockers() {
    check("is:open", &[0, 1, 4, 6, 7, 8, 9, 11, 12, 13]);
    check("is:closed", &[2, 3, 5, 10]);
    check("is:blocked", &[0, 6]);
    check("is:ready", &[1, 4, 7, 8, 9, 11, 12, 13]);
    check("is:blocked,closed", &[0, 2, 3, 5, 6, 10]);
}
