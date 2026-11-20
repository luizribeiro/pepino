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

#[test]
fn rm_returns_one_batch_outcome_for_multiple_tasks() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let output =
        as_json(&sandbox.run(&["rm", &project.blocker, &project.parent, "--yes", "--json"]));
    assert_write(&output, true);
    assert_eq!(
        output["targets"],
        serde_json::json!([project.blocker, project.parent])
    );
    assert_eq!(output["deleted"].as_array().unwrap().len(), 2);
    assert_deleted(&sandbox, &project.blocker);
    assert_deleted(&sandbox, &project.parent);
}

#[cfg(unix)]
#[test]
fn rm_leaves_other_tasks_live_when_one_is_deleted_at_the_prompt() {
    use std::{
        fs::File,
        io::{BufRead, BufReader, Read, Write},
        process::Stdio,
    };

    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let terminal = nix::pty::openpty(None, None).unwrap();
    let mut input = File::from(terminal.master);
    let mut child = sandbox
        .command()
        .args(["rm", &project.parent, &project.blocker])
        .stdin(File::from(terminal.slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut prompt = String::new();
    stderr.read_line(&mut prompt).unwrap();
    assert_eq!(prompt, "Permanently delete these tasks?\n");

    sandbox.run(&["rm", &project.blocker, "--yes"]);
    input.write_all(b"y\n").unwrap();
    let mut diagnostic = String::new();
    stderr.read_to_string(&mut diagnostic).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(diagnostic.ends_with(&format!(
        "error: Task {} \"Order stainless fittings\" was deleted\n",
        &project.blocker[..6]
    )));
    assert_eq!(show_task(&sandbox, &project.parent)["id"], project.parent);
}
