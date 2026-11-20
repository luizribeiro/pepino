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
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "Refusing to delete 1 task without confirmation\n  Renovate the greenhouse"
        )
    );
    assert!(message.contains("Will become top-level"));
    let output = sandbox
        .command()
        .args(["rm", task, &project.blocker])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Refusing to delete 2 tasks without confirmation"));
    assert!(stderr.contains(&format!(
        "Will be unblocked {}  Test the new latch",
        &project.downstream[..6]
    )));
    assert_eq!(show_task(&sandbox, task)["id"], project.parent);

    let json = as_json(&sandbox.run(&["rm", task, &project.parent, "--yes", "--json"]));
    assert_write(&json, true);
    assert_eq!(json["deleted"][0]["id"], project.parent);
    assert_eq!(json["top_level"].as_array().unwrap().len(), 2);
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
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("was deleted")
    );
}

#[test]
fn rm_resolves_all_arguments_before_deleting() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    let (ambiguous, candidates) = ambiguous_tasks(&data);
    let missing = "f".repeat(64);
    for (bad, status, code) in [
        ("bad", 2, "invalid_input"),
        (missing.as_str(), 3, "not_found"),
        (&ambiguous, 4, "ambiguous_id"),
    ] {
        let error = task_error(
            sandbox
                .command()
                .args(["rm", &project.parent, bad, "--yes", "--json"])
                .output()
                .unwrap(),
            status,
            code,
        );
        if code == "ambiguous_id" {
            assert_eq!(error["candidates"], serde_json::json!(candidates));
        }
    }
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
        assert!(prompt.contains("Will become top-level"));
        assert!(prompt.contains("Will be unblocked"));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with(expected));
        if answer == b"y\n" {
            assert!(stdout.contains("Became top-level"));
            assert!(stdout.contains("Unblocked"));
        }
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
fn rm_still_deletes_live_tasks_when_one_is_deleted_at_the_prompt() {
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
    assert!(output.status.success(), "{diagnostic}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("Deleted "));
    assert!(stdout.contains(&format!("Already deleted {}", &project.blocker[..6])));
    assert_deleted(&sandbox, &project.parent);
}

#[test]
fn rm_removes_edges_between_batch_targets_once() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let json = as_json(&sandbox.run(&["rm", &p.parent, &p.downstream, "--yes", "--json"]));
    let edges = json["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 3);
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge["kind"] == "parent" && edge["from"] == p.downstream)
            .count(),
        1
    );
    assert!(edges.iter().all(|edge| edge["effective"] == false));
    assert_eq!(
        show_task(&sandbox, &p.blocker)["parent"],
        serde_json::Value::Null
    );
}
