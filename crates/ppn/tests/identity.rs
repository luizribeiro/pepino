mod common;

use common::*;

#[test]
fn changing_the_display_name_works_in_a_pipe() {
    let (sandbox, _) = task_sandbox();
    let before = as_json(&sandbox.run(&["identity", "show", "--json"]));
    let initial_name = before["identity"]["display_name"].as_str().unwrap();
    assert!(sandbox.run(&["identity", "show"]).contains(initial_name));
    let renamed = as_json(&sandbox.run(&["identity", "rename", " Ada Workshop ", "--json"]));
    assert_eq!(renamed["schema"], 1);
    assert_eq!(renamed["identity"]["display_name"], "Ada Workshop");
    assert!(sandbox.run(&["identity", "show"]).contains("Ada Workshop"));
    let shown = as_json(&sandbox.run(&["identity", "show", "--json"]));
    assert_eq!(shown, renamed);
    assert!(
        sandbox
            .run(&["identity", "rename", "Ada Greenhouse"])
            .contains("Ada Greenhouse")
    );
    task_error(
        sandbox
            .command()
            .args(["identity", "rename", "Ada\nWorkshop", "--json"])
            .output()
            .unwrap(),
        2,
        "invalid_input",
    );
    let (empty, _) = uninitialized_task_sandbox();
    task_error(
        empty
            .command()
            .args(["identity", "show", "--json"])
            .output()
            .unwrap(),
        1,
        "not_initialized",
    );
}
