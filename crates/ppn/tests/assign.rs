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

#[test]
fn assign_reports_an_unknown_person_at_the_reference() {
    let (sandbox, _) = task_sandbox();
    let task = create_task(&sandbox, "Inspect the greenhouse latch", &[]);
    let output = sandbox
        .command()
        .args([
            "assign",
            &task,
            "Unregistered greenhouse colleague",
            "--json",
        ])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["field"], "assignees[0]");
    assert_eq!(
        error["hint"],
        "Use `me`, a display name, or an identity ID or prefix."
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("Unregistered greenhouse colleague")
    );
}

#[test]
fn assign_and_filters_agree_on_names_prefixes_and_ambiguity() {
    let (sandbox, _) = task_sandbox();
    let task = create_task(&sandbox, "Inspect the greenhouse frame", &[]);
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let name = status["identity"]["display_name"].as_str().unwrap();
    let id = status["identity"]["id"].as_str().unwrap();
    assert!(
        sandbox
            .run(&["assign", &task, name])
            .starts_with("Assigned ")
    );
    let filter = format!("assignee:{}", &id[..8]);
    assert_eq!(
        as_json(&sandbox.run(&["ls", &filter, "--json"]))["total"],
        1
    );
    assert!(
        sandbox
            .run(&["assign", "--remove", &task, &id[..8]])
            .starts_with("Unassigned ")
    );

    let a = "41ab9101".repeat(8);
    let b = "41ab9102".repeat(8);
    sandbox.run(&["assign", &task, &a, &b]);
    let write = sandbox
        .command()
        .args(["assign", &task, "41ab91", "--json"])
        .output()
        .unwrap();
    let query = sandbox
        .command()
        .args(["ls", "assignee:41ab91", "--json"])
        .output()
        .unwrap();
    let write = task_error(write, 2, "invalid_input");
    let query = task_error(query, 2, "invalid_input");
    assert_eq!(write["reason"], query["reason"]);
    assert!(
        write["reason"]
            .as_str()
            .unwrap()
            .contains("ambiguous person")
    );
}
