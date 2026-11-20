mod common;
use common::*;

#[test]
fn start_reports_changed_and_unchanged_tasks() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let human = sandbox.run(&["start", &project.parent[..8]]);
    assert!(human.starts_with("Started "));
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "doing");
    let mixed = sandbox.run(&["start", &project.parent, &project.blocker]);
    assert!(mixed.starts_with("Already started "));
    assert!(mixed.lines().nth(1).unwrap().starts_with("Started "));
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "doing");

    let json = as_json(&sandbox.run(&["start", &project.parent, &project.blocker, "--json"]));
    assert_write(&json, false);
    assert_eq!(
        json["targets"],
        serde_json::json!([project.parent, project.blocker])
    );
    assert_eq!(json["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(json["tasks"][0]["status"], "doing");
}

#[test]
fn start_resolves_all_arguments_before_changing_status() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["start", &project.parent, bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "todo");
}

#[test]
fn reopen_reports_changed_and_unchanged_tasks() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    sandbox.run(&["done", &project.parent, &project.blocker]);
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "done");
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "done");
    let human = sandbox.run(&["reopen", &project.parent[..8]]);
    assert!(human.starts_with("Reopened "));
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "todo");
    let mixed = sandbox.run(&["reopen", &project.parent, &project.blocker]);
    assert!(mixed.starts_with("Already open "));
    assert!(mixed.lines().nth(1).unwrap().starts_with("Reopened "));
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "todo");

    let json = as_json(&sandbox.run(&["reopen", &project.parent, &project.blocker, "--json"]));
    assert_write(&json, false);
    assert_eq!(
        json["targets"],
        serde_json::json!([project.parent, project.blocker])
    );
    assert_eq!(json["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(json["tasks"][0]["status"], "todo");
    sandbox.run(&["start", &project.parent]);
    assert!(
        sandbox
            .run(&["reopen", &project.parent])
            .starts_with("Already open ")
    );
    let doing = as_json(&sandbox.run(&["reopen", &project.parent, "--json"]));
    assert_write(&doing, false);
    assert_eq!(doing["tasks"][0]["status"], "doing");
    sandbox.run(&["cancel", &project.parent]);
    assert!(
        sandbox
            .run(&["reopen", &project.parent])
            .starts_with("Reopened ")
    );
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "todo");
}

#[test]
fn reopen_resolves_all_arguments_before_changing_status() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    sandbox.run(&["done", &project.parent, &project.blocker]);
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "done");
    assert_eq!(show_task(&sandbox, &project.blocker)["status"], "done");
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["reopen", &project.parent, bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(show_task(&sandbox, &project.parent)["status"], "done");
}
