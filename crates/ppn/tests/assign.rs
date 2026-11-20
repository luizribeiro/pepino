mod common;
use common::*;

#[test]
fn assign_adds_and_removes_membership() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let task = &project.parent[..8];
    let me = as_json(&sandbox.run(&["status", "--json"]))["identity"]["id"].clone();
    let other = "ff".repeat(32);
    let human = sandbox.run(&["assign", task, "me", other.as_str()]);
    assert!(human.starts_with("Assigned "));
    let json = as_json(&sandbox.run(&["assign", task, "me", other.as_str(), "--json"]));
    assert_write(&json, false);
    assert_eq!(json["targets"][0], project.parent);
    let current = show_task(&sandbox, task);
    for member in serde_json::json!([me, other]).as_array().unwrap() {
        assert!(current["assignees"].as_array().unwrap().contains(member));
    }

    let human = sandbox.run(&["assign", "--remove", task, "me", other.as_str()]);
    assert!(human.starts_with("Unassigned "));
    let current = show_task(&sandbox, task);
    assert_eq!(current["assignees"], serde_json::json!([]));
    let repeated = sandbox.run(&["assign", "--remove", task, "me", other.as_str()]);
    assert!(repeated.starts_with("Already unassigned "));
}

#[test]
fn assign_reports_reference_errors() {
    let (sandbox, data) = task_sandbox();
    let _project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["assign", bad, "me", "--json"])
            .output()
            .unwrap()
    });
}
