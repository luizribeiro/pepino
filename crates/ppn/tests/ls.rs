mod common;

use common::{SEED_NOW, as_json, create_task, seed_tasks, small_project, task_error, task_sandbox};

#[test]
fn ls_renders_filtered_rows_through_a_pipe() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert_eq!(
        sandbox.run(&["ls", "tag:hardware"]),
        format!(
            "{}  Order stainless fittings  todo #hardware\n",
            &project.blocker[..6]
        )
    );
    let json = as_json(&sandbox.run(&["ls", "tag:hardware", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "tasks");
    assert_eq!(json["total"], 1);
    assert_eq!(json["tasks"][0]["id"], project.blocker);
    assert_eq!(json["filter"], "tag:hardware is:open");
    assert!(json["names"].as_object().is_some());
}

#[test]
fn ls_reports_the_offending_filter_argument() {
    let (sandbox, _) = task_sandbox();
    let args = ["ls", "tag:hardware", "tags:garden"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(human.stdout.is_empty());
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("tags:garden"));
    assert!(stderr.contains("argument 1"));
    assert!(stderr.contains("hint: Use \"tag\""));
    let json = sandbox.command().args(args).arg("--json").output().unwrap();
    let error = task_error(json, 2, "invalid_input");
    assert_eq!(error["term"], "tags:garden");
    assert_eq!(error["position"]["index"], 1);
}

#[test]
fn ls_defaults_to_open_and_accepts_explicit_closed_filters() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    let closed = create_task(
        &sandbox,
        "Measure the greenhouse door",
        &["--status", "done"],
    );
    let cancelled = create_task(&sandbox, "Reuse the bent hinge", &["--status", "cancelled"]);
    assert_eq!(listing(&sandbox, &[])["total"], 3);
    assert_eq!(listing(&sandbox, &["--all"])["total"], 5);
    assert_eq!(
        listing(&sandbox, &["status:done"])["tasks"][0]["id"],
        closed
    );
    assert_eq!(listing(&sandbox, &["is:closed"])["total"], 2);
    assert_eq!(listing(&sandbox, &["--", "-status:todo"])["total"], 2);
    assert_eq!(
        listing(&sandbox, &["--", "-status:todo"])["tasks"][1]["id"],
        cancelled
    );
}

#[test]
fn ls_sorts_and_limits_before_rendering() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let flags = ["--sort", "created,-due", "--limit", "1"];
    let output = sandbox.command().arg("ls").args(flags).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stderr, b"showing 1 of 3\n");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "{}  Renovate the greenhouse  todo #garden\n",
            &project.parent[..6]
        )
    );
    let result = listing(&sandbox, &flags);
    assert_eq!(result["total"], 3);
    assert_eq!(result["sort"], "created,-due");
    assert_eq!(result["tasks"].as_array().unwrap().len(), 1);
    let reverse = listing(&sandbox, &["--sort", "-created"]);
    assert_eq!(reverse["tasks"][0]["id"], project.downstream);

    let output = sandbox
        .command()
        .args(["ls", "--sort", "deadline", "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 2, "invalid_input")["field"], "sort");
}

#[test]
fn ls_reports_empty_results_and_routes_an_explicit_board() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    let output = sandbox
        .command()
        .args(["ls", "tag:plumbing"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"No matching tasks.\n");
    assert_eq!(
        listing(&sandbox, &["tag:plumbing"])["tasks"],
        serde_json::json!([])
    );
    assert_eq!(listing(&sandbox, &["--board", "Personal"])["total"], 3);
    let output = sandbox
        .command()
        .args(["ls", "--board", "Workshop, Annex", "--json"])
        .output()
        .unwrap();
    assert_eq!(
        task_error(output, 3, "not_found")["board"],
        "Workshop, Annex"
    );
}

fn listing(sandbox: &common::ConfigSandbox, args: &[&str]) -> serde_json::Value {
    as_json(&sandbox.run(&[&["ls", "--json"], args].concat()))
}

#[test]
fn ls_searches_comments_and_resolves_relative_dates() {
    use chrono::{Datelike, NaiveDate, Weekday};
    use pepino_app::{Request, WriteRequest};

    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    seed_tasks(&data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Comment {
                task: project.downstream.clone(),
                text: "Check for a hairline crack before painting.".into(),
            }),
            SEED_NOW,
        )
        .await
        .unwrap();
    });

    let text = listing(&sandbox, &["HAIRLINE CRACK"]);
    assert_eq!(text["total"], 1);
    assert_eq!(text["tasks"][0]["id"], project.downstream);
    assert_eq!(text["tasks"][0]["comment_count"], 1);

    let old = create_task(
        &sandbox,
        "Renew the garden permit",
        &["--due", "2000-01-01"],
    );
    create_task(
        &sandbox,
        "Replace the greenhouse frame",
        &["--due", "2099-01-01"],
    );
    let due = listing(&sandbox, &["due:<fri"]);
    assert_eq!(due["total"], 1);
    assert_eq!(due["tasks"][0]["id"], old);
    assert_eq!(
        due["resolved_dates"]["fri"]
            .as_str()
            .unwrap()
            .parse::<NaiveDate>()
            .unwrap()
            .weekday(),
        Weekday::Fri
    );
}

#[test]
fn ls_renders_the_app_display_name_for_assignees() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    let assigned = create_task(
        &sandbox,
        "Inspect the greenhouse hinges",
        &["--tag", "hardware", "--assign", "me"],
    );
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let me = status["identity"]["id"].as_str().unwrap();
    let name = status["identity"]["display_name"].as_str().unwrap();
    let human = sandbox.run(&["ls", "assignee:me"]);
    assert_eq!(
        human,
        format!(
            "{}  Inspect the greenhouse hinges  todo #hardware @{name}\n",
            &assigned[..6]
        )
    );
    let result = listing(&sandbox, &["assignee:me"]);
    assert_eq!(result["names"][me], name);
}
