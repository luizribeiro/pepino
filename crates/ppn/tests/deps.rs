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
