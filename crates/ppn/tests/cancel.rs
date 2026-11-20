mod common;
use common::*;

#[test]
fn cancel_reports_changed_and_unchanged_tasks() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let human = sandbox.run(&["cancel", &project.parent[..8]]);
    assert!(human.starts_with("Cancelled "));
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "cancelled");
    let mixed = sandbox.run(&["cancel", &project.parent, &project.blocker]);
    assert!(mixed.starts_with("Already cancelled "));
    assert!(mixed.lines().nth(1).unwrap().starts_with("Cancelled "));
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "cancelled");

    let json = as_json(&sandbox.run(&["cancel", &project.parent, &project.blocker, "--json"]));
    assert_write(&json, false);
    assert_eq!(
        json["targets"],
        serde_json::json!([project.parent, project.blocker])
    );
    assert_eq!(json["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(json["tasks"][0]["status"], "cancelled");
}

#[test]
fn cancel_resolves_all_arguments_before_changing_status() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["cancel", &project.parent, bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "todo");
}
