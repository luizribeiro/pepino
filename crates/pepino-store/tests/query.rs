#[path = "query/fixture.rs"]
mod fixture;

#[test]
fn fixture_keeps_creation_order_across_devices() {
    let tasks = fixture::board().tasks();
    assert_eq!(tasks[0].id, fixture::id(1));
    assert_eq!(tasks[0].title, "Order hinges");
    assert_eq!(tasks[1].id, fixture::id(2));
    assert_eq!(tasks.len(), 4);
}
