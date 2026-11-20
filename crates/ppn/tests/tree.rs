mod common;

use common::{as_json, create_task, small_project, task_error, task_sandbox};

#[test]
fn tree_shows_a_forest_and_omits_closed_tasks_by_default() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let closed = create_task(
        &sandbox,
        "Unpack the fittings",
        &["--parent", &project.blocker, "--status", "done"],
    );
    let leaf = create_task(&sandbox, "Count the spare washers", &["--parent", &closed]);
    let root = create_task(&sandbox, "Prune the apple trees", &[]);
    let expected = format!(
        "{}  Renovate the greenhouse  todo #garden\n  {}  Order stainless fittings  todo #hardware\n    {}  Count the spare washers  todo\n  {}  Test the new latch  todo · blocked\n{}  Prune the apple trees  todo\n",
        &project.parent[..6],
        &project.blocker[..6],
        &leaf[..6],
        &project.downstream[..6],
        &root[..6],
    );
    assert_eq!(sandbox.run(&["tree"]), expected);

    let json = as_json(&sandbox.run(&["tree", &project.blocker, "--all", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "tree");
    assert_eq!(json["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(json["nodes"][0]["depth"], 0);
    assert_eq!(json["nodes"][1]["task"]["id"], closed);
    assert_eq!(json["nodes"][2]["depth"], 2);
    assert_eq!(json["nodes"][2]["task"]["id"], leaf);
    let missing = "0".repeat(64);
    let output = sandbox
        .command()
        .args(["tree", &missing, "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["task"], missing);
}
