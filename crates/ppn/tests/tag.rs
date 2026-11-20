mod common;
use common::*;

#[test]
fn tag_adds_and_removes_membership() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let task = &project.parent[..8];
    let other = "tools";
    let human = sandbox.run(&["tag", task, "hardware", other]);
    assert!(human.starts_with("Tagged "));
    let json = as_json(&sandbox.run(&["tag", task, "hardware", other, "--json"]));
    assert_write(&json, false);
    assert_eq!(json["targets"][0], project.parent);
    let current = show_task(&sandbox, task);
    for member in serde_json::json!(["hardware", "tools"]).as_array().unwrap() {
        assert!(current["tags"].as_array().unwrap().contains(member));
    }

    let human = sandbox.run(&["tag", "--remove", task, "hardware", other]);
    assert!(human.starts_with("Untagged "));
    let current = show_task(&sandbox, task);
    assert_eq!(current["tags"], serde_json::json!(["garden"]));
    let repeated = sandbox.run(&["tag", "--remove", task, "hardware", other]);
    assert!(repeated.starts_with("Already untagged "));
}

#[test]
fn tag_reports_reference_errors() {
    let (sandbox, data) = task_sandbox();
    let _project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["tag", bad, "hardware", "--json"])
            .output()
            .unwrap()
    });
}
