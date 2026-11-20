use serde_json::json;

mod common;
use common::*;

#[test]
fn show_renders_the_protocol_task_and_configured_due_date() {
    let (sandbox, _) = task_sandbox();
    let write = as_json(&sandbox.run(&[
        "add",
        "Check the greenhouse hinges",
        "--due",
        "2026-12-01",
        "--json",
    ]));
    let id = write["created"].as_str().unwrap();
    let human = sandbox.run(&["show", &id[..6], "--color", "always"]);
    assert!(human.starts_with(&format!(
        "{}  Check the greenhouse hinges  todo\n",
        &id[..6]
    )));
    assert!(human.contains("due 01 Dec 2026\n"));
    assert!(!human.contains("Comments"));
    assert!(!human.contains(" UTC #"));
    let mut detail = as_json(&sandbox.run(&["show", id, "--json"]));
    let names = detail.as_object_mut().unwrap().remove("names").unwrap();
    assert_eq!(names, write["names"]);
    let mut task = write["tasks"][0].clone();
    task["comments"] = json!([]);
    assert_eq!(
        detail,
        json!({"schema":1,"kind":"task","task":task,"edges":[]})
    );
}

#[test]
fn show_diagnostics_distinguish_malformed_and_missing_references() {
    let (sandbox, _) = task_sandbox();
    let error = |args: &[&str], status, code| {
        let output = sandbox.command().args(args).arg("--json").output().unwrap();
        task_error(output, status, code)
    };
    let missing = error(&["show", "deadbeef"], 3, "not_found");
    assert_eq!(missing["field"], "task");
    assert_eq!(missing["task"], "deadbeef");
    assert_eq!(missing["message"], "No task matches deadbeef");
    assert!(missing["hint"].as_str().unwrap().contains("ppn ls"));
    let malformed = error(&["show", "latch"], 2, "invalid_input");
    assert!(malformed["message"].as_str().unwrap().contains("\"latch\""));
    assert!(malformed["hint"].as_str().unwrap().contains("4 hex digits"));
}

#[test]
fn show_reports_deleted_tasks_as_conflicts() {
    use pepino_app::{Request, WriteRequest};
    let (sandbox, data) = task_sandbox();
    let id = create_task(&sandbox, "Order the new mounting plate", &[]);
    seed_tasks(&data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Delete { task: id.clone() }),
            SEED_NOW,
        )
        .await
        .unwrap();
    });
    let output = sandbox
        .command()
        .args(["show", &id[..6], "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 1, "conflict");
    assert_eq!(error["field"], "task");
    assert_eq!(error["task"], id);
}

#[test]
fn show_reports_ambiguous_ids_with_sorted_candidates() {
    use pepino_app::{Request, Response};
    use ppn::write_args::new_task;
    use std::collections::BTreeMap;

    let (sandbox, data) = task_sandbox();
    let (prefix, mut expected) = seed_tasks(&data, async |app| {
        let mut seen = BTreeMap::new();
        for number in 0..=65_536 {
            let input = new_task(format!("Inspect hinge assembly {number}"));
            let Response::Write {
                created: Some(id), ..
            } = app.handle(Request::Write(input), SEED_NOW).await.unwrap()
            else {
                panic!("expected a created task");
            };
            let prefix = id[..4].to_owned();
            if let Some(previous) = seen.insert(prefix.clone(), id.clone()) {
                return (prefix, [previous, id]);
            }
        }
        panic!("65,537 tasks must share a four-digit prefix");
    });
    expected.sort();
    let output = sandbox
        .command()
        .args(["show", &prefix, "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 4, "ambiguous_id");
    assert_eq!(error["field"], "task");
    assert_eq!(error["count"], 2);
    assert_eq!(error["candidates"], json!(expected));
    let human = sandbox.command().args(["show", &prefix]).output().unwrap();
    assert_eq!(human.status.code(), Some(4));
    let human = String::from_utf8(human.stderr).unwrap();
    assert!(expected.iter().all(|id| human.contains(id)));
}

#[cfg(unix)]
#[test]
fn show_uses_tty_dates_and_configured_color() {
    let (sandbox, data) = task_sandbox();
    std::fs::write(
        &sandbox.config,
        format!(
            "data_dir = {}\n[ui]\ncolor = 'always'\ndate_format = '%d %b %Y'\n",
            json!(data)
        ),
    )
    .unwrap();
    let id = create_task(&sandbox, "Inspect the hinge", &["--due", "tomorrow"]);
    let plain = sandbox.run(&["show", &id, "--color", "always"]);
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("due ") && line.as_bytes()[4].is_ascii_digit())
    );
    let args = ["show", &id, "--config", sandbox.config.to_str().unwrap()];
    let colored = String::from_utf8(tty_output(&args, None)).unwrap();
    assert!(colored.contains("\x1b[36m"));
    assert!(colored.contains("due tomorrow") || colored.contains("due today"));
    let suppressed = String::from_utf8(tty_output(&args, Some("1"))).unwrap();
    assert!(!suppressed.contains('\x1b'));
}
