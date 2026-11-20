mod common;

use common::{SEED_NOW, as_json, create_task, seed_tasks, small_project, task_error, task_sandbox};

#[test]
fn ls_renders_filtered_rows_through_a_pipe() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert_eq!(
        sandbox.run(&["ls", "tag:hardware"]),
        format!(
            "{} todo  Order stainless fittings  #hardware\n",
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
            "{} todo  Renovate the greenhouse  #garden\n",
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
            "{} todo  Inspect the greenhouse hinges  #hardware @{name}\n",
            &assigned[..6]
        )
    );
    let result = listing(&sandbox, &["assignee:me"]);
    assert_eq!(result["names"][me], name);
}

#[test]
fn negated_terms_and_flags_can_be_interleaved() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    assert!(
        sandbox
            .run(&["ls", "tag:garden", "-is:blocked"])
            .starts_with(&project.parent[..6])
    );
    let output = sandbox.run(&[
        "ls",
        "-tag:someday",
        "--limit",
        "5",
        "-is:blocked",
        "--json",
    ]);
    assert_eq!(as_json(&output)["total"], 2);
    let output = sandbox.run(&[
        "ls",
        "--sort",
        "-created",
        "--all",
        "-tag:someday",
        "-h",
        "--json",
    ]);
    assert_eq!(as_json(&output)["name"], "ls");
    assert_eq!(
        as_json(&sandbox.run(&["ready", "-tag:hardware", "--json"]))["total"],
        1
    );
    let output = sandbox.run(&[
        "view",
        "save",
        "weekend",
        "-tag:someday",
        "--json",
        "-is:blocked",
    ]);
    assert_eq!(
        as_json(&output)["view"]["filter"],
        "-tag:someday -is:blocked"
    );
}

#[test]
fn ls_warns_on_unknown_custom_fields_without_failing() {
    let (sandbox, _) = task_sandbox();
    let args = ["ls", "statuss:todo"];
    let output = sandbox.command().args(args).output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"No matching tasks.\nwarning: Unknown filter field \"statuss\". Did you mean \"status\"?\n");
    let output = sandbox.command().args(args).arg("--json").output().unwrap();
    assert!(output.status.success());
    let warning = as_json(std::str::from_utf8(&output.stderr).unwrap());
    assert_eq!(warning["kind"], "warning");
    assert_eq!(warning["schema"], 1);
    assert_eq!(warning["warning"]["code"], "unknown_filter_field");
    assert_eq!(
        as_json(std::str::from_utf8(&output.stdout).unwrap())["warnings"],
        serde_json::json!([
            {"field": "statuss", "suggestion": "status"}
        ])
    );
}

#[test]
fn ls_applies_status_terms_in_nested_saved_views() {
    let (sandbox, _) = task_sandbox();
    small_project(&sandbox);
    let closed = create_task(&sandbox, "Paint the shed", &["--status", "done"]);
    sandbox.run(&["view", "save", "finished", "is:closed"]);
    sandbox.run(&["view", "save", "shed", "@finished"]);
    assert!(sandbox.run(&["ls", "@shed"]).contains("Paint the shed"));
    let result = listing(&sandbox, &["@shed"]);
    assert_eq!(result["tasks"][0]["id"], closed);
    assert_eq!(result["total"], 1);
    assert_eq!(result["constrains_status"], true);
    assert_eq!(listing(&sandbox, &["-is:blocked"])["total"], 2);
}

#[cfg(unix)]
#[test]
fn ls_aligns_status_title_and_metadata_on_a_terminal() {
    let (sandbox, _) = task_sandbox();
    create_task(
        &sandbox,
        "Call the plumber",
        &["--due", "2026-11-20", "--priority", "3"],
    );
    create_task(
        &sandbox,
        "Check the shutoff valve",
        &["--status", "doing", "--priority", "4"],
    );
    let piped = sandbox.run(&["ls", "--sort", "title"]);
    assert!(
        piped
            .lines()
            .next()
            .unwrap()
            .ends_with("todo  Call the plumber  due:2026-11-20 p3")
    );
    let mut command = sandbox.command();
    command.args(["ls", "--sort", "title", "--color", "never"]);
    let output = common::success(common::task_terminal(command, Some(b"")));
    let rows: Vec<_> = output.lines().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].find("todo"), rows[1].find("doing"));
    assert_eq!(
        rows[0].find("Call the plumber"),
        rows[1].find("Check the shutoff valve")
    );
    assert_eq!(rows[0].find("due:"), rows[1].find("p4"));
}

#[test]
fn numeric_filter_reports_text_values_in_stdout_and_stderr_json() {
    let (sandbox, _) = task_sandbox();
    for (title, value) in [("Order hinges", "cost=12.50"), ("Order screws", "cost=14")] {
        create_task(&sandbox, title, &["--set", value]);
    }
    let output = sandbox
        .command()
        .args(["ls", "cost:>10", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let listing = as_json(&String::from_utf8(output.stdout).unwrap());
    assert_eq!(listing["total"], 1);
    assert_eq!(listing["warnings"][0]["text_tasks"], 1);
    let warning = as_json(&String::from_utf8(output.stderr).unwrap());
    assert_eq!(warning["warning"]["code"], "numeric_filter_on_text");
    assert_eq!(
        warning["warning"]["message"],
        listing["warnings"][0]["message"]
    );
}

#[test]
fn default_sort_places_negative_priority_below_unprioritized_tasks() {
    let (sandbox, _) = task_sandbox();
    let low = create_task(&sandbox, "Paint the trim", &["--priority=-2"]);
    let middle = create_task(&sandbox, "Inspect the frame", &[]);
    let high = create_task(&sandbox, "Seal the roof", &["--priority=3"]);
    let listed = as_json(&sandbox.run(&["ls", "--json"]));
    assert_eq!(listed["tasks"][0]["id"], high);
    assert_eq!(listed["tasks"][1]["id"], middle);
    assert_eq!(listed["tasks"][2]["id"], low);
    let explicit = as_json(&sandbox.run(&["ls", "--sort=-priority", "--json"]));
    assert_eq!(explicit["tasks"][1]["id"], low);
}

#[test]
fn a_task_prefix_in_a_filter_has_the_standard_ambiguity_exit_status() {
    let (sandbox, data) = task_sandbox();
    let (prefix, candidates) = common::ambiguous_tasks(&data);
    let term = format!("parent:{prefix}");
    let output = sandbox
        .command()
        .args(["ls", &term, "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 4, "ambiguous_id");
    assert_eq!(error["task"], prefix);
    assert_eq!(error["term"], term);
    assert_eq!(error["candidates"], serde_json::json!(candidates));
}

#[test]
fn listing_accepts_canonical_filters_and_combines_both_grammars() {
    let (sandbox, _) = task_sandbox();
    let kept = create_task(&sandbox, "Repair the greenhouse hinge", &["-t", "hardware"]);
    create_task(&sandbox, "Replace the bad hinge", &["-t", "hardware"]);
    let original = as_json(&sandbox.run(&["ls", "tag:hardware", "", "--json"]));
    assert_eq!(original["total"], 2);
    let canonical = original["filter"].as_str().unwrap();
    let roundtrip = as_json(&sandbox.run(&["ls", "--filter", canonical, "--json"]));
    assert_eq!(original["tasks"], roundtrip["tasks"]);
    let combined =
        as_json(&sandbox.run(&["ready", "-f", r#"-"bad hinge""#, "tag:hardware", "--json"]));
    assert_eq!(combined["total"], 1);
    assert_eq!(combined["tasks"][0]["id"], kept);
}

#[test]
fn keyed_positional_filter_errors_explain_how_to_pass_multiple_terms() {
    let (sandbox, _) = task_sandbox();
    let args = ["ls", "tag:hardware tag:garden"];
    let output = sandbox.command().args(args).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(
        message.contains("separate arguments or use --filter"),
        "{message}"
    );
    let error = task_error(
        sandbox.command().args(args).arg("--json").output().unwrap(),
        2,
        "invalid_input",
    );
    assert!(error["hint"].as_str().unwrap().contains("--filter"));
}

#[test]
fn list_and_delete_are_visible_working_aliases() {
    let (sandbox, _) = task_sandbox();
    let id = create_task(&sandbox, "Repair the greenhouse latch", &["-t", "hardware"]);
    assert_eq!(
        sandbox.run(&["list", "-tag:outdoors", "--json"]),
        sandbox.run(&["ls", "-tag:outdoors", "--json"])
    );
    let help = as_json(&common::run_ppn(&["help", "--json"]));
    for (name, alias) in [("ls", "list"), ("rm", "delete")] {
        let command = help["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == name)
            .unwrap();
        assert_eq!(command["aliases"], serde_json::json!([alias]));
    }
    let deleted = as_json(&sandbox.run(&["delete", &id, "--yes", "--json"]));
    assert_eq!(deleted["deleted"][0]["id"], id);
}
