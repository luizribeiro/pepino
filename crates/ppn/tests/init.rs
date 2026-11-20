mod common;
use common::*;

#[test]
fn init_names_the_local_records_and_is_idempotent() {
    let (sandbox, data) = uninitialized_task_sandbox();
    let output = sandbox.run(&[
        "init",
        "--name",
        "Ada Gardener",
        "--device",
        "Garden laptop",
        "--board",
        "Greenhouse",
    ]);
    assert!(output.contains("Identity: Ada Gardener"));
    assert!(output.contains("Device: Garden laptop"));
    assert!(output.contains("Board: Greenhouse"));
    let before = std::fs::read(data.join("local.json")).unwrap();
    let status = sandbox.run(&["status", "--json"]);
    let repeated = sandbox.run(&[
        "init",
        "--name",
        "Another name",
        "--board",
        "Another board",
        "--json",
    ]);
    assert_eq!(as_json(&status), as_json(&repeated));
    assert_eq!(std::fs::read(data.join("local.json")).unwrap(), before);
}
