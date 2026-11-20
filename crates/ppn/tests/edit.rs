mod common;
use common::*;

#[test]
fn edit_applies_scalar_and_membership_flags_in_one_result() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let task = &p.parent[..8];
    let args = [
        "edit",
        task,
        "--title",
        "Rebuild the greenhouse",
        "--description",
        "Buy cedar",
        "--due",
        "2026-12-01",
        "--priority",
        "-3",
        "--status",
        "doing",
        "--set",
        "estimate=4",
        "--tag",
        "carpentry",
        "--untag",
        "garden",
        "--assign",
        "me",
    ];
    let human = sandbox.run(&args);
    assert!(human.starts_with("Edited "));
    assert_eq!(human.lines().count(), 1);

    let edited = show_task(&sandbox, task);
    assert_eq!(edited["title"], "Rebuild the greenhouse");
    assert_eq!(edited["description"], "Buy cedar");
    assert_eq!(edited["due"], "2026-12-01");
    assert_eq!(edited["priority"], -3);
    assert_eq!(edited["status"], "doing");
    assert_eq!(edited["fields"]["estimate"]["value"], 4);
    assert_eq!(edited["tags"], serde_json::json!(["carpentry"]));
    assert_eq!(edited["assignees"].as_array().unwrap().len(), 1);

    let json = as_json(&sandbox.run(&[args.as_slice(), &["--json"]].concat()));
    assert_write(&json, false);
    assert_eq!(json["targets"], serde_json::json!([p.parent]));
    assert!(sandbox.run(&args).starts_with("Already edited "));
}
