mod common;
use common::*;

#[test]
fn done_reports_changed_and_unchanged_tasks() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let human = sandbox.run(&["done", &project.parent[..8]]);
    assert!(human.starts_with("Done "));
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "done");
    let mixed = sandbox.run(&["done", &project.parent, &project.blocker]);
    assert!(mixed.starts_with("Already done "));
    assert!(mixed.lines().nth(1).unwrap().starts_with("Done "));
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "done");

    let json = as_json(&sandbox.run(&["done", &project.parent, &project.blocker, "--json"]));
    assert_write(&json, false);
    assert_eq!(
        json["targets"],
        serde_json::json!([project.parent, project.blocker])
    );
    assert_eq!(json["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(json["tasks"][0]["status"], "done");
}

#[test]
fn done_resolves_all_arguments_before_changing_status() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["done", &project.parent, bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "todo");
}
