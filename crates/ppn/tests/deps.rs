mod common;

use common::{as_json, create_task, small_project, task_error, task_sandbox};

#[test]
fn deps_deduplicates_the_shared_blocker_in_a_diamond() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let shared = create_task(
        &sandbox,
        "Check the delivery",
        &["--status", "done", "--blocks", &project.blocker],
    );
    let other = create_task(&sandbox, "Inspect the hinge", &["--blocked-by", &shared]);
    let target = create_task(
        &sandbox,
        "Open the greenhouse",
        &["--blocked-by", &project.blocker, "--blocked-by", &other],
    );

    let mut direct = [
        format!(
            "  {} todo  Order stainless fittings  #hardware\n",
            &project.blocker[..6]
        ),
        format!("  {} todo  Inspect the hinge\n", &other[..6]),
    ];
    direct.sort();
    let expected = format!(
        "blocked by\n{}{}    {} done  Check the delivery · no longer blocking\n",
        direct[0],
        direct[1],
        &shared[..6]
    );
    assert_eq!(sandbox.run(&["deps", &target]), expected);

    let json = as_json(&sandbox.run(&["deps", &target, "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "deps");
    assert_eq!(json["task"]["id"], target);
    assert_eq!(json["blockers"][2]["task"]["id"], shared);
    assert_eq!(json["blockers"][2]["distance"], 2);
    assert_eq!(json["blockers"].as_array().unwrap().len(), 3);
    assert_eq!(json["dependents"], serde_json::json!([]));

    let reverse = as_json(&sandbox.run(&["deps", &shared, "--json"]));
    assert!(
        reverse["dependents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["task"]["id"] == target && row["distance"] == 2)
    );
    let output = sandbox
        .command()
        .args(["deps", "bad", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 2, "invalid_input")["field"], "task");
}

#[test]
fn task_reads_keep_reference_statuses_across_graphs_and_history() {
    use pepino_app::{Request, WriteRequest};

    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    let deleted = project.downstream;
    common::seed_tasks(&data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Delete {
                task: deleted.clone(),
            }),
            common::SEED_NOW,
        )
        .await
        .unwrap();
    });
    let (prefix, candidates) = common::ambiguous_tasks(&data);

    let missing = "0".repeat(64);
    for command in ["deps", "tree", "log"] {
        for (task, status, code) in [
            ("bad", 2, "invalid_input"),
            (missing.as_str(), 3, "not_found"),
            (prefix.as_str(), 4, "ambiguous_id"),
        ] {
            let output = sandbox
                .command()
                .args([command, task, "--json"])
                .output()
                .unwrap();
            let error = task_error(output, status, code);
            assert_eq!(error["field"], "task");
            if status == 4 {
                assert_eq!(error["candidates"], serde_json::json!(candidates));
            }
        }
    }

    for command in ["deps", "tree", "log"] {
        let output = sandbox
            .command()
            .args([command, &deleted, "--json"])
            .output()
            .unwrap();
        assert_eq!(task_error(output, 3, "not_found")["task"], deleted);
    }
    let log = as_json(&sandbox.run(&["log", "--all", "--json"]));
    let events = log["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["change"]["action"] == "deleted"
                && event["change"]["task"] == deleted)
    );
}

#[test]
fn deps_prints_an_empty_state_only_on_human_stderr() {
    let (sandbox, _) = task_sandbox();
    let task = create_task(&sandbox, "Paint the shed", &[]);
    let output = sandbox.command().args(["deps", &task]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"No dependencies for Paint the shed.\n");
    let result = as_json(&sandbox.run(&["deps", &task, "--json"]));
    assert_eq!(result["blockers"], serde_json::json!([]));
    assert_eq!(result["dependents"], serde_json::json!([]));
}
