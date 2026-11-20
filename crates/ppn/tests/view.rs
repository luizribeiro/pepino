mod common;

use common::{as_json, small_project, task_error, task_sandbox};
use serde_json::json;

#[test]
fn view_saves_lists_shows_and_applies_a_filter() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert_eq!(sandbox.run(&["view", "ls"]), "");
    assert_eq!(
        sandbox.run(&["view", "save", "weekend", "tag:hardware"]),
        "Saved weekend\n"
    );
    assert_eq!(sandbox.run(&["view", "ls"]), "weekend  tag:hardware\n");
    assert_eq!(
        sandbox.run(&["view", "show", "weekend"]),
        "weekend  tag:hardware\n"
    );
    let definition = json!({"name":"weekend","filter":"tag:hardware"});
    assert_eq!(
        as_json(&sandbox.run(&["view", "ls", "--json"])),
        json!({"schema":1,"kind":"views","views":[definition]})
    );
    assert_eq!(
        as_json(&sandbox.run(&["view", "show", "weekend", "--json"])),
        json!({"schema":1,"kind":"view","view":definition})
    );
    let tasks = as_json(&sandbox.run(&["ls", "@weekend", "--json"]));
    assert_eq!(tasks["total"], 1);
    assert_eq!(tasks["tasks"][0]["id"], project.blocker);
    let output = sandbox
        .command()
        .args(["view", "show", "missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["view"], "missing");
}

#[test]
fn view_can_replace_and_remove_a_filter() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let args = ["view", "save", "weekend"];
    let created = as_json(&sandbox.run(&[&args[..], &["tag:hardware", "--json"]].concat()));
    assert_eq!(
        created,
        json!({"schema":1,"kind":"view_saved","view":{"name":"weekend","filter":"tag:hardware"},"replaced":false})
    );
    let replaced = as_json(&sandbox.run(&[&args[..], &["tag:garden", "--json"]].concat()));
    let definition = json!({"name":"weekend","filter":"tag:garden"});
    assert_eq!(
        replaced,
        json!({"schema":1,"kind":"view_saved","view":definition,"replaced":true})
    );
    let tasks = as_json(&sandbox.run(&["ls", "@weekend", "--json"]));
    assert_eq!(tasks["tasks"][0]["id"], project.parent);
    assert_eq!(
        as_json(&sandbox.run(&["view", "rm", "weekend", "--json"])),
        json!({"schema":1,"kind":"view_deleted","view":definition})
    );
    let output = sandbox
        .command()
        .args(["view", "rm", "weekend", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["view"], "weekend");
}

#[test]
fn view_removing_a_referenced_filter_requires_force() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    sandbox.run(&["view", "save", "weekend", "tag:hardware"]);
    sandbox.run(&["view", "save", "maintenance", "@weekend"]);
    let args = ["view", "rm", "weekend"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    assert!(
        String::from_utf8(human.stderr)
            .unwrap()
            .contains("hint: Pass `--force`")
    );
    let output = sandbox.command().args(args).arg("--json").output().unwrap();
    let error = task_error(output, 1, "conflict");
    assert_eq!(error["view"], "weekend");
    assert_eq!(error["referenced_by"], json!(["maintenance"]));
    assert!(error["hint"].as_str().unwrap().contains("--force"));

    assert_eq!(
        sandbox.run(&["view", "rm", "weekend", "--force"]),
        "Removed weekend\n"
    );
    assert_eq!(sandbox.run(&["view", "ls"]), "maintenance  @weekend\n");
    let output = sandbox
        .command()
        .args(["ls", "@maintenance", "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["term"], "@weekend");
    assert_eq!(error["view_chain"], json!(["maintenance"]));
}

#[test]
fn view_requires_a_filter_and_preserves_argument_diagnostics() {
    let (sandbox, _) = task_sandbox();
    let output = sandbox
        .command()
        .args(["view", "save", "weekend", "--json"])
        .output()
        .unwrap();
    task_error(output, 2, "usage");
    let output = sandbox
        .command()
        .args([
            "view",
            "save",
            "weekend",
            "tag:garden",
            "tags:hardware",
            "--json",
        ])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["term"], "tags:hardware");
    assert_eq!(error["position"]["index"], 1);
    assert_eq!(error["hint"], "Use \"tag\"");

    let output = sandbox
        .command()
        .args(["view", "show", "Weekend Plan", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 2, "invalid_input")["field"], "name");
}
