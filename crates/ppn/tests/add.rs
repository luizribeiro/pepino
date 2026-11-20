use serde_json::{Value, json};

mod common;
use common::*;

#[test]
fn add_reports_the_created_task_through_the_binary() {
    let (sandbox, _) = task_sandbox();
    let human = sandbox.run(&["add", "Order the replacement latch"]);
    assert!(human.starts_with("Created "));
    assert!(human.ends_with("  Order the replacement latch\n"));
    assert_eq!(human.split_whitespace().nth(1).unwrap().len(), 6);

    let write = as_json(&sandbox.run(&["add", "Check the hinges", "--json"]));
    assert_eq!(write["schema"], 1);
    assert_eq!(write["kind"], "write");
    assert_eq!(write["changed"], true);
    assert_eq!(write["edges"], json!([]));
    assert_eq!(write["tasks"][0]["id"], write["created"]);
    assert_eq!(write["created"].as_str().unwrap().len(), 64);
    assert_eq!(write["tasks"][0]["title"], "Check the hinges");
}

#[test]
fn add_reads_title_or_description_from_stdin() {
    let (sandbox, _) = task_sandbox();
    let title = success(task_stdin(
        &sandbox,
        &["add", "-"],
        "Inspect the roof latch\r\n",
    ));
    assert!(title.ends_with("  Inspect the roof latch\n"));
    let description = "Install the larger hinge.\nReuse the existing screws.\n";
    let write = as_json(&success(task_stdin(
        &sandbox,
        &[
            "add",
            "Install the larger hinge",
            "--description",
            "-",
            "--json",
        ],
        description,
    )));
    assert_eq!(
        write["tasks"][0]["description"],
        description.trim_end_matches('\n')
    );
    let collision = sandbox
        .command()
        .args(["add", "-", "--description", "-", "--json"])
        .output()
        .unwrap();
    let error = assert_json_error(collision, 2, "usage");
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--from-file")
    );
}

#[test]
fn add_accepts_protocol_fields_on_json_stdin() {
    let (sandbox, _) = task_sandbox();
    let result = task_stdin(
        &sandbox,
        &["add", "-", "--json", "--priority", "2", "--tag", "spring"],
        r#"{"title":"Repair the gate","description":"Check the packing slip.\r\n","status":"doing","tags":["hardware"],"priority":1}"#,
    );
    let task = &as_json(&success(result))["tasks"][0];
    assert_eq!(task["title"], "Repair the gate");
    assert_eq!(task["status"], "doing");
    assert_eq!(task["priority"], 2);
    assert_eq!(task["description"], "Check the packing slip.");
    assert_eq!(task["tags"], json!(["hardware", "spring"]));
    let malformed = task_stdin(
        &sandbox,
        &["add", "--json", "-"],
        r#"{"title":"Check the board","typo":true}"#,
    );
    assert_eq!(malformed.status.code(), Some(2));
    let error = serde_json::from_slice::<Value>(&malformed.stderr).unwrap();
    assert_eq!(error["error"]["field"], "stdin");
    assert_eq!(error["error"]["code"], "invalid_input");
}

#[test]
fn task_flags_round_trip_through_add_show_and_status() {
    let (sandbox, _) = task_sandbox();
    let Project {
        parent,
        blocker,
        downstream,
    } = small_project(&sandbox);
    let description = "Use stainless hardware.\nReuse the right hinge.";
    let file = sandbox.home.path().join("description.txt");
    std::fs::write(&file, description).unwrap();
    let foreign = "f0".repeat(32);
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let me = status["identity"]["id"].as_str().unwrap();
    let write = as_json(&sandbox.run(&[
        "add",
        "Replace the greenhouse latch",
        "-t",
        "greenhouse",
        "--tag",
        "hardware",
        "--due",
        "2026-12-01",
        "--priority",
        "2",
        "--assign",
        "me",
        "--assign",
        &foreign,
        "--set",
        "estimate=3",
        "--set",
        "delivery=2026-11-30",
        "--set",
        "finish=stainless",
        "--set",
        "obsolete=5",
        "--unset",
        "obsolete",
        "--parent",
        &parent,
        "--blocks",
        &downstream,
        "--blocked-by",
        &blocker,
        "--from-file",
        file.to_str().unwrap(),
        "--json",
    ]));
    let id = write["created"].as_str().unwrap();
    let task = write["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap();
    let mut assignees = [me, foreign.as_str()];
    assignees.sort_unstable();
    assert_eq!(
        *task,
        json!({
            "id":id,"title":"Replace the greenhouse latch","description":description,
            "status":"todo","due":"2026-12-01","priority":2,"tags":["greenhouse","hardware"],
            "assignees":assignees,"fields":{"estimate":{"type":"integer","value":3},
                "delivery":{"type":"date","value":"2026-11-30"},"finish":{"type":"text","value":"stainless"}},
            "created":task["created"],"updated":task["updated"],"author":status["device"]["id"],
            "parent":parent,"children":[],"blocks":[downstream],"blocked_by":[blocker],"relates":[],
            "blocked":true,"ready":false,"comment_count":0
        })
    );
    for clock in [&task["created"], &task["updated"]] {
        assert_eq!(clock.as_object().unwrap().len(), 3);
        let utc = chrono::DateTime::parse_from_rfc3339(clock["utc"].as_str().unwrap()).unwrap();
        assert!(clock["utc"].as_str().unwrap().ends_with('Z'));
        assert_eq!(clock["wall_ms"], utc.timestamp_millis());
        assert!(clock["counter"].is_u64());
    }
    for key in ["schema", "kind", "changed", "created", "tasks", "edges"] {
        assert!(write.get(key).is_some(), "{key} missing from {write}");
    }
    assert_eq!(write["edges"].as_array().unwrap().len(), 3);
    assert!(
        write["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["effective"] == true)
    );
    let human = sandbox.run(&["show", id]);
    for text in [
        description.lines().next().unwrap(),
        "Order stainless fittings",
        "Renovate the greenhouse",
        "Test the new latch",
        "due 01 Dec 2026",
        "estimate: 3",
        status["identity"]["display_name"].as_str().unwrap(),
    ] {
        assert!(human.contains(text), "{human}");
    }
    let mut task = task.clone();
    task["comments"] = json!([]);
    let mut detail = as_json(&sandbox.run(&["show", id, "--json"]));
    let names = detail.as_object_mut().unwrap().remove("names").unwrap();
    assert_eq!(names, write["names"]);
    assert_eq!(
        detail,
        json!({"schema":1,"kind":"task","task":task,"edges":[],
            "relation_titles":{parent:"Renovate the greenhouse", blocker:"Order stainless fittings",
                downstream:"Test the new latch"},
            "assignee_names":{me:status["identity"]["display_name"]}
        })
    );
    assert!(sandbox.run(&["status"]).contains("Tasks: 4 open, 0 closed"));
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["counts"]["open"],
        4
    );
}

#[test]
fn simultaneous_add_processes_share_one_board() {
    use std::process::Stdio;
    let (sandbox, _) = task_sandbox();
    let spawn = |title| {
        sandbox
            .command()
            .args(["add", title, "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn("Inspect the greenhouse roof");
    let second = spawn("Replace the broken panel");
    let first = as_json(&success(first.wait_with_output().unwrap()));
    let second = as_json(&success(second.wait_with_output().unwrap()));
    assert_ne!(first["created"], second["created"]);
    for write in [first, second] {
        let id = write["created"].as_str().unwrap();
        let detail = as_json(&sandbox.run(&["show", id, "--json"]));
        assert_eq!(detail["task"]["id"], id);
    }
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["counts"]["open"],
        2
    );
}

#[test]
fn add_rejects_cyclic_dependencies_in_human_output() {
    let (sandbox, _) = task_sandbox();
    let end = create_task(&sandbox, "Seal the frame", &[]);
    let start = create_task(&sandbox, "Sand the rough edges", &["--blocks", &end]);
    let output = sandbox
        .command()
        .args([
            "add",
            "Paint the frame",
            "--blocks",
            &start,
            "--blocked-by",
            &end,
            "--color",
            "always",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.starts_with("error: "));
    assert!(error.contains("would create a cycle"));
    assert!(!error.contains('\x1b'));
    assert!(!sandbox.run(&["ls"]).contains("Paint the frame"));
}

#[test]
fn add_diagnostics_report_invalid_values_and_hints() {
    let (sandbox, _) = task_sandbox();
    let error = |args: &[&str], status, code| {
        let output = sandbox.command().args(args).arg("--json").output().unwrap();
        task_error(output, status, code)
    };
    let invalid = error(
        &["add", "Repair the latch", "-t", "bad tag"],
        2,
        "invalid_input",
    );
    assert_eq!(invalid["field"], "tags[0]");
    assert!(invalid["hint"].as_str().unwrap().contains("[a-z]"));
    assert!(
        invalid["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid tag \"bad tag\":")
    );
    let due = error(
        &["add", "Repair the latch", "--due", "Friday"],
        2,
        "invalid_input",
    );
    assert_eq!(due["field"], "due");
    assert!(due["hint"].as_str().unwrap().contains("lowercase weekday"));
}

#[test]
fn write_date_and_field_operands_match_the_app_filter() {
    use pepino_app::{FilterInput, ReadRequest, Request, Response};

    let (sandbox, data) = task_sandbox();
    let id = create_task(
        &sandbox,
        "Inspect the latch",
        &[
            "--due",
            "-2w",
            "--set",
            "anchor=today",
            "--set",
            "arrival=tomorrow",
            "--set",
            "part=\"004\"",
            "--set",
            "estimate=+004",
        ],
    );
    let detail = as_json(&sandbox.run(&["show", &id, "--json"]));
    let today = detail["task"]["fields"]["anchor"]["value"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let filter = FilterInput::Argv(
        [
            "due:-2w",
            "arrival:tomorrow",
            "part:\"004\"",
            "estimate:+004",
        ]
        .map(String::from)
        .into(),
    );
    let response = seed_tasks(&data, async |app| {
        app.handle(
            Request::Read(ReadRequest::ListTasks {
                filter,
                sort: None,
                limit: None,
                today,
            }),
            SEED_NOW,
        )
        .await
        .unwrap()
    });
    let Response::Tasks {
        tasks,
        resolved_dates,
        ..
    } = response
    else {
        panic!("expected a listing");
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id);
    assert_eq!(resolved_dates["-2w"], tasks[0].due.unwrap());
}

#[test]
fn add_reports_cyclic_dependencies_in_json() {
    let (sandbox, _) = task_sandbox();
    let end = create_task(&sandbox, "Seal the frame", &[]);
    let start = create_task(&sandbox, "Sand the rough edges", &["--blocks", &end]);
    let output = sandbox
        .command()
        .args([
            "add",
            "Paint the frame",
            "--blocks",
            &start,
            "--blocked-by",
            &end,
            "--json",
        ])
        .output()
        .unwrap();
    let error = task_error(output, 1, "conflict");
    assert_eq!(error["field"], "blocked_by[0]");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("would create a cycle")
    );

    let output = sandbox
        .command()
        .args([
            "add",
            "Paint the frame",
            "--blocks",
            &start,
            "--blocked-by",
            &start[..8],
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 1, "conflict")["field"], "blocked_by[0]");
}

#[test]
fn add_clears_stdin_due_dates_with_unset() {
    let (sandbox, _) = task_sandbox();
    let output = task_stdin(
        &sandbox,
        &["add", "-", "--unset", "due", "--json"],
        r#"{"title":"Inspect the latch","due":"2026-12-01"}"#,
    );
    assert_eq!(as_json(&success(output))["tasks"][0]["due"], Value::Null);
}

#[test]
fn add_rejects_empty_and_multiline_titles() {
    let (sandbox, _) = task_sandbox();
    for title in ["  ", "Inspect the latch\nOrder another hinge"] {
        let error = task_error(
            sandbox
                .command()
                .args(["add", title, "--json"])
                .output()
                .unwrap(),
            2,
            "invalid_input",
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains(&format!("{title:?}"))
        );
        assert!(error["hint"].is_null());
    }
    assert!(
        sandbox
            .run(&["add", "  Inspect the latch  "])
            .ends_with("  Inspect the latch\n")
    );
}
