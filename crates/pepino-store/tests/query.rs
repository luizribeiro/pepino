#[path = "query/fixture.rs"]
mod fixture;

#[test]
fn fixture_keeps_creation_order_across_devices() {
    let tasks = fixture::board().tasks();
    assert_eq!(tasks[0].id, fixture::id(1));
    assert_eq!(tasks[0].title, "Order hinges");
    assert_eq!(tasks[1].id, fixture::id(2));
    assert_eq!(tasks.len(), 14);
    assert!(fixture::board().task(fixture::id(14)).is_none());
    assert_eq!(
        fixture::board().task(fixture::id(1)).unwrap().parent,
        Some(fixture::id(7))
    );
    assert_eq!(
        fixture::board().comments(fixture::id(0))[0].text,
        "Bring galvanized bolts"
    );
    let fence = fixture::board().task(fixture::id(0)).unwrap();
    assert_eq!(fence.description.as_deref(), Some("Use CÈDRE posts"));
    assert_eq!(fence.custom.len(), 3);
}
