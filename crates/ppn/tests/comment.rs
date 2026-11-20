mod common;
use common::*;

#[test]
fn comment_appends_text_from_each_source_and_preserves_repeats() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let task = &project.blocker[..8];
    let note = "Check the packing slip.";
    assert!(
        sandbox
            .run(&["comment", task, note])
            .starts_with("Commented on ")
    );
    assert_eq!(show_task(&sandbox, task)["comments"][0]["text"], note);

    let json = as_json(&success(task_stdin(
        &sandbox,
        &["comment", task, "-", "--json"],
        note,
    )));
    assert_write(&json, true);
    assert_eq!(json["tasks"][0]["comment_count"], 2);
    assert_eq!(show_task(&sandbox, task)["comments"][1]["text"], note);

    let file = sandbox.home.path().join("fittings.txt");
    std::fs::write(&file, "Sort the bolts.\nCount the washers.\n").unwrap();
    let path = file.to_str().unwrap();
    sandbox.run(&["comment", task, "--from-file", path]);
    assert_eq!(
        show_task(&sandbox, task)["comments"][2]["text"],
        "Sort the bolts.\nCount the washers.\n"
    );
}

#[test]
fn comment_reports_reference_and_input_errors() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["comment", bad, "Check the hinges", "--json"])
            .output()
            .unwrap()
    });
    let output = sandbox
        .command()
        .args(["comment", &project.parent, "--json"])
        .output()
        .unwrap();
    task_error(output, 2, "usage");
    assert_eq!(show_task(&sandbox, &project.parent)["comment_count"], 0);
}

#[test]
fn comment_rejects_blank_arguments_and_closed_stdin() {
    let (sandbox, _) = task_sandbox();
    let id = create_task(&sandbox, "Install the greenhouse latch", &[]);
    for text in ["", " \t"] {
        let output = sandbox
            .command()
            .args(["comment", &id, text, "--json"])
            .output()
            .unwrap();
        assert_eq!(task_error(output, 2, "invalid_input")["field"], "text");
    }
    let output = task_stdin(&sandbox, &["comment", &id, "-", "--json"], "");
    assert_eq!(task_error(output, 2, "invalid_input")["field"], "text");
    assert_eq!(show_task(&sandbox, &id)["comment_count"], 0);
}
