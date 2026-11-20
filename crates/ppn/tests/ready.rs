mod common;

use common::{as_json, small_project, task_error, task_sandbox};

#[test]
fn ready_lists_unblocked_tasks_with_the_same_filters_as_ls() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert_eq!(
        sandbox.run(&["ready", "tag:hardware"]),
        format!(
            "{} todo  Order stainless fittings  #hardware\n",
            &project.blocker[..6]
        )
    );
    let ready = as_json(&sandbox.run(&["ready", "--json"]));
    assert_eq!(ready["schema"], 1);
    assert_eq!(ready["kind"], "tasks");
    assert_eq!(ready["total"], 2);
    assert_eq!(ready["tasks"][0]["id"], project.parent);
    assert_eq!(ready["tasks"][1]["id"], project.blocker);
    assert_eq!(ready, as_json(&sandbox.run(&["ls", "is:ready", "--json"])));
    sandbox.run(&["start", &project.parent]);
    let ready = as_json(&sandbox.run(&["ready", "--json"]));
    assert_eq!(ready["total"], 1);
    assert_eq!(ready["tasks"][0]["id"], project.blocker);
    assert_eq!(ready, as_json(&sandbox.run(&["ls", "is:ready", "--json"])));
    let output = sandbox
        .command()
        .args(["ready", "--all", "--json"])
        .output()
        .unwrap();
    assert!(
        task_error(output, 2, "usage")["message"]
            .as_str()
            .unwrap()
            .contains("--all")
    );

    let output = sandbox
        .command()
        .args(["ready", "is:waiting", "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["term"], "is:waiting");
    assert_eq!(error["position"]["index"], 0);
}

#[test]
fn ready_help_explains_the_ready_predicate() {
    let help = as_json(&common::run_ppn(&["help", "ready", "--json"]));
    assert_eq!(help["summary"], "List tasks that are ready to start");
    assert_eq!(help["examples"][1], "ppn ready assignee:me --json");
}
