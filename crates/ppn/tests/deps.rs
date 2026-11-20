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
            "  {}  Order stainless fittings  todo #hardware\n",
            &project.blocker[..6]
        ),
        format!("  {}  Inspect the hinge  todo\n", &other[..6]),
    ];
    direct.sort();
    let expected = format!(
        "blocked by\n{}{}    {}  Check the delivery  done · no longer blocking\nblocks\n",
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
    use pepino_app::{Request, Response, WriteRequest};
    use ppn::write_args::new_task;
    use std::collections::BTreeMap;

    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    let deleted = project.downstream;
    let (prefix, mut candidates) = common::seed_tasks(&data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Delete {
                task: deleted.clone(),
            }),
            common::SEED_NOW,
        )
        .await
        .unwrap();
        let mut prefixes = BTreeMap::new();
        for n in 0..=65_536 {
            let response = app
                .handle(
                    Request::Write(new_task(format!("Inspect greenhouse hinge {n}"))),
                    common::SEED_NOW,
                )
                .await
                .unwrap();
            let Response::Write {
                created: Some(id), ..
            } = response
            else {
                panic!("expected a task creation");
            };
            let prefix = id[..4].to_owned();
            if project.parent.starts_with(&prefix) || project.blocker.starts_with(&prefix) {
                continue;
            }
            if let Some(previous) = prefixes.insert(prefix.clone(), id.clone()) {
                return (prefix, [previous, id]);
            }
        }
        panic!("four-character prefixes must collide");
    });
    candidates.sort();

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

    for command in ["deps", "tree"] {
        let output = sandbox
            .command()
            .args([command, &deleted, "--json"])
            .output()
            .unwrap();
        assert_eq!(task_error(output, 1, "conflict")["task"], deleted);
    }
    let log = as_json(&sandbox.run(&["log", &deleted, "--json"]));
    let events = log["events"].as_array().unwrap();
    assert_eq!(events.last().unwrap()["change"]["action"], "deleted");
}
