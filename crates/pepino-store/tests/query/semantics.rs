use pepino_store::query::{matches, select};

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
fn assignees_match_the_current_identity_or_people_resolved_by_the_caller() {
    check("assignee:me", &[0, 1]);
    check(r#"assignee:"Ana Maria""#, &[0, 1]);
    check(r#"assignee:"me""#, &[1, 2]);
    check("assignee:ana-work", &[0, 1]);
    check("assignee:ana,bob", &[0, 1, 2]);
    check("tag:garden -assignee:me,bob", &[7, 11]);
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
fn relations_use_resolved_prefixes_and_effective_direct_edges() {
    check("parent:0808", &[1]);
    check("parent:abcd01", &[6]);
    check("parent:0808,abcd01", &[1, 6]);
    check("blocks:abcd01", &[1]);
    check("blocks:ABCD02", &[]);
    check("blocks:0707", &[0]);
    check("blocked-by:ABCD02", &[0]);
    check("blocked-by:abcd01,0303", &[6, 8]);
}

#[test]
fn state_predicates_use_status_and_effective_blockers() {
    check("is:open", &[0, 1, 4, 6, 7, 8, 9, 11, 12, 13]);
    check("is:closed", &[2, 3, 5, 10]);
    check("is:blocked", &[0, 6]);
    check("is:ready", &[1, 4, 7, 8, 9, 11, 12, 13]);
    check("is:blocked,closed", &[0, 2, 3, 5, 6, 10]);
}

#[test]
fn presence_checks_every_field_including_empty_description_and_custom_values() {
    check("has:due", &[0, 1, 2, 6, 7, 10, 11]);
    check("has:parent", &[1, 6]);
    check("has:assignee", &[0, 1, 2]);
    check("has:priority", &[0, 1, 2, 4, 7, 8, 10, 11]);
    check("has:tag", &[0, 1, 2, 4, 6, 7, 8, 11]);
    check("has:description", &[0, 3]);
    check("has:estimate", &[0, 1]);
    check("has:budget", &[]);
    check("has:parent,description", &[0, 1, 3, 6]);
}

#[test]
fn custom_comparisons_respect_operand_type_and_lexical_case() {
    check("estimate:>9", &[0]);
    check(r#"estimate:"12""#, &[1]);
    check(r#"estimate:12,"12""#, &[0, 1]);
    check("inspection:>=2026-11-21", &[0]);
    check("inspection:>2026-11-21", &[]);
    check(r#"supplier:<"Beta""#, &[0]);
    check("supplier:acme", &[]);
    check("supplier:12", &[]);
    check("estimate:2026-11-20", &[]);
    check("budget:>0", &[]);
    check("-estimate:>=0 has:estimate", &[1]);
}

#[test]
fn free_text_searches_each_body_with_unicode_simple_case_folding() {
    check(r#""NORTH FENCE""#, &[0]);
    check("cèdre", &[0]);
    check(r#""GALVANIZED BOLTS""#, &[0]);
    check("οδυσσεύσ", &[12]);
    check("STRAẞE", &[2, 7]);
    check("STRASSE", &[]);
    check("fence bolts", &[0]);
    check("fence -bolts", &[]);
    check(r#""fence use""#, &[]);
    check("greenhouse", &[]);
}

#[test]
fn views_expand_recursively_and_negate_the_entire_saved_filter() {
    check("@weekend", &[0, 7, 11]);
    check("tag:garden -@weekend", &[2]);
    check("@weekend @garden-open @weekend", &[0, 7, 11]);
    check("-@empty", &[]);
    let filter = filter("@empty -tag:garden");
    let task = board().task(id(1)).unwrap();
    assert!(matches(board(), task, &query(&filter)).unwrap());
}
