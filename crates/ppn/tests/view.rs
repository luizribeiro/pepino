mod common;

use common::{as_json, small_project, task_error, task_sandbox};
use serde_json::json;

#[test]
fn view_saves_lists_shows_and_applies_a_filter() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert_eq!(sandbox.run(&["view", "ls"]), "");
    assert_eq!(
        sandbox.run(&["view", "save", "weekend", "tag:hardware"]),
        "Saved weekend\n"
    );
    assert_eq!(sandbox.run(&["view", "ls"]), "weekend  tag:hardware\n");
    assert_eq!(
        sandbox.run(&["view", "show", "weekend"]),
        "weekend  tag:hardware\n"
    );
    let definition = json!({"name":"weekend","filter":"tag:hardware"});
    assert_eq!(
        as_json(&sandbox.run(&["view", "ls", "--json"])),
        json!({"schema":1,"kind":"views","views":[definition]})
    );
    assert_eq!(
        as_json(&sandbox.run(&["view", "show", "weekend", "--json"])),
        json!({"schema":1,"kind":"view","view":definition})
    );
    let tasks = as_json(&sandbox.run(&["ls", "@weekend", "--json"]));
    assert_eq!(tasks["total"], 1);
    assert_eq!(tasks["tasks"][0]["id"], project.blocker);
    let output = sandbox
        .command()
        .args(["view", "show", "missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["view"], "missing");
}
