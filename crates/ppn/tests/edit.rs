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

#[test]
fn edit_reads_descriptions_from_stdin_and_files() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let note = "Tighten each hinge.\nCheck the clearance.\n";
    let output = task_stdin(
        &sandbox,
        &["edit", &p.blocker, "--description", "-", "--json"],
        note,
    );
    assert_write(&as_json(&success(output)), true);
    assert_eq!(show_task(&sandbox, &p.blocker)["description"], note);

    let path = sandbox.home.path().join("fittings.txt");
    std::fs::write(&path, "Use countersunk screws.").unwrap();
    sandbox.run(&["edit", &p.blocker, "--from-file", path.to_str().unwrap()]);
    assert_eq!(
        show_task(&sandbox, &p.blocker)["description"],
        "Use countersunk screws."
    );
}

#[test]
fn edit_reports_reference_errors_and_requires_flags_without_a_terminal() {
    let (sandbox, data) = task_sandbox();
    let p = small_project(&sandbox);
    assert_task_reference_errors(&sandbox, &data, |bad| {
        sandbox
            .command()
            .args(["edit", bad, "--title", "Rebuild the door", "--json"])
            .output()
            .unwrap()
    });
    let output = sandbox
        .command()
        .env("VISUAL", "false")
        .args(["edit", &p.parent, "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 2, "usage");
    let hint = error["hint"].as_str().unwrap();
    for flag in [
        "--title",
        "--description",
        "--from-file",
        "--due",
        "--priority",
        "--status",
        "--set",
        "--unset",
        "--tag",
        "--untag",
        "--assign",
        "--unassign",
    ] {
        assert!(hint.contains(flag));
    }
    assert_eq!(
        show_task(&sandbox, &p.parent)["title"],
        "Renovate the greenhouse"
    );
}

#[test]
fn edit_clears_optional_fields_and_unassigns() {
    let (sandbox, _) = task_sandbox();
    let task = create_task(
        &sandbox,
        "Install a greenhouse brace",
        &[
            "--description",
            "Use stainless bolts.",
            "--due",
            "2026-12-01",
            "--priority",
            "3",
            "--set",
            "estimate=4",
            "--assign",
            "me",
        ],
    );
    let task = &task[..8];
    let json = as_json(&sandbox.run(&[
        "edit",
        task,
        "--unset",
        "description",
        "--unset",
        "due",
        "--unset",
        "priority",
        "--unset",
        "estimate",
        "--unassign",
        "me",
        "--json",
    ]));
    assert_write(&json, true);
    assert_eq!(json["tasks"][0]["assignees"], serde_json::json!([]));
    assert!(json["tasks"][0]["description"].is_null());
    let edited = show_task(&sandbox, task);
    for field in ["description", "due", "priority"] {
        assert!(edited[field].is_null());
    }
    assert_eq!(edited["fields"], serde_json::json!({}));
    assert_eq!(edited["assignees"], serde_json::json!([]));
}
