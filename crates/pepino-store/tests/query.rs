#[path = "query/fixture.rs"]
mod fixture;
#[path = "query/sort.rs"]
mod sort;

#[test]
fn missing_identity_has_a_typed_error() {
    use pepino_store::query::{QueryError, QueryErrorKind};
    let error = QueryError {
        term: "assignee:me".into(),
        kind: QueryErrorKind::MissingIdentity,
    };
    assert_eq!(error.to_string(), "assignee:me: no current identity for me");
}
