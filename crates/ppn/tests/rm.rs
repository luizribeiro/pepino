mod common;
use common::*;

#[test]
fn rm_requires_yes_through_a_pipe_and_reports_permanent_deletion() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let task = &project.parent[..8];
    let error = task_error(
        sandbox
            .command()
            .args(["rm", task, "--json"])
            .output()
            .unwrap(),
        1,
        "confirmation_required",
    );
    assert_eq!(error["hint"], "Pass --yes to confirm.");
    assert_eq!(
        error["message"],
        "Refusing to delete 1 task without confirmation\n  Renovate the greenhouse"
    );
    let output = sandbox
        .command()
        .args(["rm", task, &project.blocker])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stderr, b"error: Refusing to delete 2 tasks without confirmation\n  Renovate the greenhouse\n  Order stainless fittings\nhint: Pass --yes to confirm.\n");
    assert_eq!(show_task(&sandbox, task)["id"], project.parent);

    let json = as_json(&sandbox.run(&["rm", task, &project.parent, "--yes", "--json"]));
    assert_write(&json, true);
    assert_eq!(json["deleted"][0]["id"], project.parent);
    assert_eq!(json["deleted"].as_array().unwrap().len(), 1);
    assert_deleted(&sandbox, task);
    task_error(
        sandbox
            .command()
            .args(["rm", task, "--yes", "--json"])
            .output()
            .unwrap(),
        1,
        "conflict",
    );
}

fn assert_deleted(sandbox: &ConfigSandbox, task: &str) {
    let output = sandbox
        .command()
        .args(["show", task, "--json"])
        .output()
        .unwrap();
    task_error(output, 1, "conflict");
}

#[test]
fn rm_resolves_all_arguments_before_deleting() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["rm", &project.parent, bad, "--yes", "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(
        show_task(&sandbox, &project.parent)["title"],
        "Renovate the greenhouse"
    );
}

#[cfg(unix)]
#[test]
fn rm_lists_titles_and_accepts_yes_and_no_on_a_terminal() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    for (answer, expected) in [(b"n\n", "Deletion cancelled."), (b"y\n", "Deleted ")] {
        let mut command = sandbox.command();
        command.args(["rm", &project.parent, &project.blocker]);
        let output = task_terminal(command, Some(answer));
        assert!(output.status.success(), "{output:?}");
        let prompt = String::from_utf8(output.stderr).unwrap();
        assert!(prompt.contains("Renovate the greenhouse"));
        assert!(prompt.contains("Order stainless fittings"));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with(expected));
        if answer == b"n\n" {
            assert_eq!(show_task(&sandbox, &project.parent)["id"], project.parent);
            assert_eq!(show_task(&sandbox, &project.blocker)["id"], project.blocker);
        }
    }
    assert_deleted(&sandbox, &project.parent);
    assert_deleted(&sandbox, &project.blocker);
}
