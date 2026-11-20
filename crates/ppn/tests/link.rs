mod common;
use common::*;

#[test]
fn link_orients_each_kind_and_reports_an_existing_link() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let human = sandbox.run(&["link", &p.parent[..8], "blocks", &p.blocker]);
    assert!(human.starts_with("Linked "));
    assert_eq!(show_task(&sandbox, &p.blocker)["blocked_by"][0], p.parent);

    let json = as_json(&sandbox.run(&["link", &p.downstream, "blocked-by", &p.parent, "--json"]));
    assert_write(&json, true);
    assert_eq!(json["edges"][0]["from"], p.parent);
    assert_eq!(json["edges"][0]["to"], p.downstream);
    let task = show_task(&sandbox, &p.downstream);
    assert!(
        task["blocked_by"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(p.parent))
    );
    assert!(
        sandbox
            .run(&["link", &p.parent, "blocks", &p.downstream])
            .starts_with("Already linked ")
    );

    sandbox.run(&["link", &p.parent, "relates", &p.blocker]);
    assert_eq!(show_task(&sandbox, &p.blocker)["relates"][0], p.parent);
    let template = create_task(&sandbox, "Make a drill template", &[]);
    sandbox.run(&["link", &template, "parent", &p.blocker]);
    assert_eq!(show_task(&sandbox, &template)["parent"], p.blocker);
}
