mod common;
use common::*;

#[test]
fn unlink_removes_relationships_in_sentence_order() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let task = &p.downstream[..8];
    let human = sandbox.run(&["unlink", task, "blocked-by", &p.blocker]);
    assert!(human.starts_with("Unlinked "));
    assert_eq!(
        show_task(&sandbox, task)["blocked_by"],
        serde_json::json!([])
    );

    let json = as_json(&sandbox.run(&["unlink", &p.blocker, "blocks", task, "--json"]));
    assert_write(&json, false);
    assert_eq!(json["edges"], serde_json::json!([]));
    assert!(
        sandbox
            .run(&["unlink", task, "blocked-by", &p.blocker])
            .starts_with("Already unlinked ")
    );

    let json = as_json(&sandbox.run(&["unlink", task, "parent", &p.parent, "--json"]));
    assert_write(&json, true);
    assert_eq!(json["edges"][0]["effective"], false);
    assert!(json["edges"][0].get("reason").is_none());
    assert!(show_task(&sandbox, task)["parent"].is_null());

    sandbox.run(&["link", &p.parent, "relates", task]);
    assert_eq!(show_task(&sandbox, task)["relates"][0], p.parent);
    sandbox.run(&["unlink", task, "relates", &p.parent]);
    assert_eq!(show_task(&sandbox, task)["relates"], serde_json::json!([]));
}

#[test]
fn unlink_validates_both_endpoints() {
    let (sandbox, data) = task_sandbox();
    let p = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["unlink", &p.parent, "relates", bad, "--json"])
            .output()
            .unwrap()
    });
    assert_eq!(
        show_task(&sandbox, &p.parent)["relates"],
        serde_json::json!([])
    );
}
