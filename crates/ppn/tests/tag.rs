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

#[test]
fn membership_commands_explain_the_single_task_argument() {
    let (sandbox, _) = task_sandbox();
    let first = create_task(&sandbox, "Fit the hinges", &[]);
    let second = create_task(&sandbox, "Replace the latch", &[]);
    for (name, input) in [("tag", "hardware"), ("assign", "me")] {
        let args = [name, &first, &second, input];
        let output = sandbox.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(&format!("ppn {name} <TASK>")), "{stderr}");
        let output = sandbox.command().args(args).arg("--json").output().unwrap();
        assert!(
            task_error(output, 2, "usage")["hint"]
                .as_str()
                .unwrap()
                .contains("one task first")
        );
    }
}

#[test]
fn untag_and_unassign_are_visible_removal_commands() {
    let (sandbox, _) = task_sandbox();
    let id = create_task(
        &sandbox,
        "Paint the greenhouse door",
        &["-t", "hardware", "--assign", "me"],
    );
    let human = sandbox.run(&["untag", &id, "hardware"]);
    assert!(human.starts_with("Untagged "));
    let json = as_json(&sandbox.run(&["unassign", &id, "me", "--json"]));
    assert_write(&json, true);
    assert_eq!(json["tasks"][0]["tags"], serde_json::json!([]));
    assert_eq!(json["tasks"][0]["assignees"], serde_json::json!([]));
    let help = run_ppn(&["--help"]);
    assert!(help.contains("untag"));
    assert!(help.contains("unassign"));
}
