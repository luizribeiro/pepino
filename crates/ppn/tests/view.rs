mod common;

use common::{as_json, small_project, task_error, task_sandbox};
use serde_json::json;

#[test]
fn view_saves_lists_shows_and_applies_a_filter() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let empty = sandbox.command().args(["view", "ls"]).output().unwrap();
    assert!(empty.status.success());
    assert!(empty.stdout.is_empty());
    assert_eq!(empty.stderr, b"No saved views.\n");
    assert_eq!(
        as_json(&sandbox.run(&["view", "ls", "--json"])),
        json!({"schema":1,"kind":"views","views":[]})
    );
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
    assert_eq!(
        sandbox.run(&["ls", "@weekend"]),
        sandbox.run(&["ls", "tag:hardware"])
    );
    let output = sandbox
        .command()
        .args(["view", "show", "missing", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["view"], "missing");
}

#[test]
fn view_can_replace_and_remove_a_filter() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let args = ["view", "save", "weekend"];
    let created = as_json(&sandbox.run(&[&args[..], &["tag:hardware", "--json"]].concat()));
    assert_eq!(
        created,
        json!({"schema":1,"kind":"view_saved","view":{"name":"weekend","filter":"tag:hardware"},"replaced":false})
    );
    let replaced = as_json(&sandbox.run(&[&args[..], &["tag:garden", "--json"]].concat()));
    let definition = json!({"name":"weekend","filter":"tag:garden"});
    assert_eq!(
        replaced,
        json!({"schema":1,"kind":"view_saved","view":definition,"replaced":true})
    );
    assert_eq!(
        sandbox.run(&["view", "save", "weekend", "tag:garden"]),
        "Replaced weekend\n"
    );
    let tasks = as_json(&sandbox.run(&["ls", "@weekend", "--json"]));
    assert_eq!(tasks["tasks"][0]["id"], project.parent);
    assert_eq!(
        as_json(&sandbox.run(&["view", "rm", "weekend", "--json"])),
        json!({"schema":1,"kind":"view_deleted","view":definition})
    );
    let output = sandbox
        .command()
        .args(["view", "rm", "weekend", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["view"], "weekend");
}

#[test]
fn view_removing_a_referenced_filter_requires_force() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    sandbox.run(&["view", "save", "weekend", "tag:hardware"]);
    sandbox.run(&["view", "save", "maintenance", "@weekend"]);
    let args = ["view", "rm", "weekend"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    assert!(
        String::from_utf8(human.stderr)
            .unwrap()
            .contains("hint: Pass `--force`")
    );
    let output = sandbox.command().args(args).arg("--json").output().unwrap();
    let error = task_error(output, 1, "conflict");
    assert_eq!(error["view"], "weekend");
    assert_eq!(error["referenced_by"], json!(["maintenance"]));
    assert!(error["hint"].as_str().unwrap().contains("--force"));

    assert_eq!(
        sandbox.run(&["view", "rm", "weekend", "--force"]),
        "Removed weekend\n"
    );
    assert_eq!(sandbox.run(&["view", "ls"]), "maintenance  @weekend\n");
    let output = sandbox
        .command()
        .args(["ls", "@maintenance", "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["term"], "@weekend");
    assert_eq!(error["view_chain"], json!(["maintenance"]));
}

#[test]
fn view_requires_a_filter_and_preserves_argument_diagnostics() {
    let (sandbox, _) = task_sandbox();
    let output = sandbox
        .command()
        .args(["view", "save", "weekend", "--json"])
        .output()
        .unwrap();
    task_error(output, 2, "usage");
    let output = sandbox
        .command()
        .args([
            "view",
            "save",
            "weekend",
            "tag:garden",
            "tags:hardware",
            "--json",
        ])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["term"], "tags:hardware");
    assert_eq!(error["position"]["index"], 1);
    assert_eq!(error["hint"], "Use \"tag\"");

    let output = sandbox
        .command()
        .args(["view", "show", "Weekend Plan", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 2, "invalid_input")["field"], "name");
}

#[test]
fn view_save_accepts_a_full_filter_with_or_without_positional_terms() {
    let (sandbox, _) = task_sandbox();
    let filter = r#"tag:hardware -"bad hinge""#;
    let saved = as_json(&sandbox.run(&["view", "save", "weekend", "-f", filter, "--json"]));
    assert_eq!(saved["view"]["filter"], filter);
    let combined = as_json(&sandbox.run(&[
        "view", "save", "weekend", "--filter", filter, "is:open", "--json",
    ]));
    assert_eq!(combined["view"]["filter"], format!("{filter} is:open"));
    assert_eq!(combined["replaced"], true);
}

#[test]
fn an_unknown_view_suggests_a_cli_command_without_repeating_the_reason() {
    let (sandbox, _) = task_sandbox();
    let args = ["ls", "@missing"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(2));
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("ppn view ls"));
    assert!(!stderr.contains("list_views"));
    let json = sandbox.command().args(args).arg("--json").output().unwrap();
    let error = task_error(json, 2, "invalid_input");
    assert!(error.get("reason").is_none());
}

#[test]
fn saved_filter_errors_point_to_the_supplied_filter_string() {
    let (sandbox, _) = task_sandbox();
    let output = sandbox
        .command()
        .args([
            "view",
            "save",
            "weekend",
            "--filter",
            "  @missing",
            "--json",
        ])
        .output()
        .unwrap();
    let error = task_error(output, 2, "invalid_input");
    assert_eq!(error["input"], "--filter");
    assert_eq!(
        error["position"],
        serde_json::json!({"kind":"byte","byte":2})
    );
    assert_eq!(error["view_chain"], serde_json::json!(["weekend"]));
}
#[test]
fn legacy_views_migrate_once_and_keep_a_conflicting_filter() {
    let (sandbox, data) = task_sandbox();
    small_project(&sandbox);
    sandbox.run(&["device", "rename", "Garden laptop"]);
    sandbox.run(&["view", "save", "weekend", "tag:hardware"]);
    sandbox.run(&["view", "save", "same", "is:open"]);
    sandbox.run(&["view", "save", "removed", "tag:hardware"]);
    sandbox.run(&["view", "rm", "removed"]);

    let path = data.join("local.json");
    let mut local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    local["saved_views"] = json!({
        "weekend": "tag:garden",
        "same": "is:open",
        "removed": "tag:garden",
        "maintenance": "@weekend",
    });
    std::fs::write(&path, serde_json::to_vec(&local).unwrap()).unwrap();

    let listing = as_json(&sandbox.run(&["view", "ls", "--json"]));
    let saved = listing["views"].as_array().unwrap();
    assert_eq!(saved.len(), 5);
    assert!(saved.contains(&json!({"name":"weekend","filter":"tag:hardware"})));
    assert!(saved.contains(&json!({"name":"same","filter":"is:open"})));
    assert!(saved.contains(&json!({"name":"maintenance","filter":"@weekend"})));
    assert!(!saved.iter().any(|row| row["name"] == "removed"));
    let recovered = saved
        .iter()
        .find(|row| row["recovered_as"] == "weekend (from Garden laptop)")
        .unwrap();
    let recovery = recovered["name"].as_str().unwrap();
    assert_eq!(recovered["filter"], "tag:garden");
    assert_eq!(
        sandbox.run(&["view", "show", recovery]),
        format!("{recovery}  tag:garden\n")
    );
    let reference = format!("@{recovery}");
    assert_eq!(
        sandbox.run(&["ls", &reference]),
        sandbox.run(&["ls", "tag:garden"])
    );
    let human = sandbox.run(&["view", "ls"]);
    assert!(human.contains(&format!(
        "weekend (from Garden laptop) [{reference}]  tag:garden"
    )));

    let bytes = std::fs::read(&path).unwrap();
    let migrated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(migrated.get("saved_views").is_none());
    assert_eq!(as_json(&sandbox.run(&["view", "ls", "--json"])), listing);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn view_list_reports_unimported_legacy_names_without_blocking_other_commands() {
    let (sandbox, data) = task_sandbox();
    let path = data.join("local.json");
    let mut local: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let huge = "x".repeat(5000);
    local["saved_views"] = json!({"Bad Name":"is:open","huge":huge,"repairs":"tag:hardware"});
    std::fs::write(&path, serde_json::to_vec(&local).unwrap()).unwrap();

    let human = sandbox.command().args(["view", "ls"]).output().unwrap();
    assert!(human.status.success());
    assert_eq!(human.stdout, b"repairs  tag:hardware\n");
    assert_eq!(
        human.stderr,
        b"warning: Could not import saved views: Bad Name, huge. Remove with `ppn view rm NAME` or save again under a valid name.\n"
    );
    let json = sandbox
        .command()
        .args(["view", "ls", "--json"])
        .output()
        .unwrap();
    assert!(json.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json.stdout).unwrap()["not_imported"],
        json!(["Bad Name", "huge"])
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&json.stderr).unwrap()["kind"],
        "warning"
    );

    assert_eq!(
        sandbox.run(&["view", "show", "Bad Name"]),
        "Bad Name  is:open\n"
    );
    let shown = as_json(&sandbox.run(&["view", "show", "huge", "--json"]));
    assert_eq!(shown["view"], json!({"name":"huge","filter":huge}));
    let removed = as_json(&sandbox.run(&["view", "rm", "Bad Name", "--json"]));
    assert_eq!(
        removed["view"],
        json!({"name":"Bad Name","filter":"is:open"})
    );
    assert_eq!(sandbox.run(&["view", "rm", "huge"]), "Removed huge\n");
    for action in ["show", "rm"] {
        let missing = sandbox
            .command()
            .args(["view", action, "huge", "--json"])
            .output()
            .unwrap();
        assert_eq!(task_error(missing, 3, "not_found")["view"], "huge");
    }
    let cleared = sandbox
        .command()
        .args(["view", "ls", "--json"])
        .output()
        .unwrap();
    assert!(cleared.status.success());
    assert!(cleared.stderr.is_empty());
    let current: serde_json::Value = serde_json::from_slice(&cleared.stdout).unwrap();
    assert!(current.get("not_imported").is_none());
    assert_eq!(
        current["views"],
        json!([{"name":"repairs","filter":"tag:hardware"}])
    );
    sandbox.run(&["status"]);
}
