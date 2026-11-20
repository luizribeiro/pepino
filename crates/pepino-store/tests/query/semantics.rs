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
