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
    let human = sandbox.run(&["show", &id[..6]]);
    assert!(human.starts_with(&format!(
        "{}  Check the greenhouse hinges  todo\n",
        &id[..6]
    )));
    assert!(human.contains("due 01 Dec 2026\n"));
    assert!(!human.contains("Comments"));
    assert!(!human.contains(" UTC #"));
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let person = status["identity"]["display_name"].as_str().unwrap();
    let device = status["device"]["name"].as_str().unwrap();
    assert!(human.contains(&format!(" by {person} ·")));
    assert!(
        sandbox
            .run(&["show", id, "--verbose"])
            .contains(&format!(" by {person} ({device}) ·"))
    );

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
fn show_reports_deleted_tasks_as_not_found() {
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
    let error = task_error(output, 3, "not_found");
    assert_eq!(error["field"], "task");
    assert_eq!(error["task"], id);
    let output = sandbox.command().args(["show", &id[..6]]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "error: Task {} \"Order the new mounting plate\" was deleted\n  input <TASK>\n",
            &id[..6]
        )
    );
}

#[test]
fn show_reports_ambiguous_ids_with_sorted_candidates() {
    let (sandbox, data) = task_sandbox();
    let (prefix, expected) = ambiguous_tasks(&data);
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
    let ids = ppn::render::ShortIds::new(expected.iter().map(String::as_str));
    assert_eq!(
        human,
        format!(
            "error: Task reference {prefix} matches 2 tasks\n  {}\n  {}\n  input <TASK>\nhint: Use a longer task ID.\n",
            ids.get(&expected[0]),
            ids.get(&expected[1])
        )
    );
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
    let plain = sandbox.run(&["show", &id, "--color", "never"]);
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("due ") && line.as_bytes()[4].is_ascii_digit())
    );
    let show = |flags: &[&str], no_color: &str| {
        let mut command = sandbox.command();
        command
            .args(["show", &id])
            .args(flags)
            .env("NO_COLOR", no_color);
        let output = task_terminal(command, None);
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{output:?}"
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let colored = show(&[], "");
    assert!(colored.contains("\x1b[36m"));
    assert!(colored.contains("due tomorrow") || colored.contains("due today"));
    assert_eq!(show(&[], "1"), colored);
    assert!(!show(&["--color", "auto"], "1").contains('\x1b'));
}

#[test]
fn validation_and_lookup_errors_have_one_message() {
    let (sandbox, _) = task_sandbox();
    for (args, status, expected) in [
        (
            &["add", "   "][..],
            2,
            "error: invalid title \"   \": expected a non-empty title\n  input <TITLE>\n",
        ),
        (
            &["ls", "--board", "Workshop"],
            3,
            "error: board Workshop was not found\n  input --board\n",
        ),
        (
            &["view", "rm", "weekend"],
            3,
            "error: view weekend was not found\n  input <NAME>\n",
        ),
        (
            &["show", "latch"],
            2,
            "error: invalid task reference \"latch\": expected 4 to 64 hexadecimal digits\n  input <TASK>\nhint: Use an id shown by `ppn ls`, at least 4 hex digits.\n",
        ),
    ] {
        let output = sandbox.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(status));
        assert_eq!(output.stderr, expected.as_bytes());
    }
}

#[test]
fn human_errors_shorten_full_ids_while_json_keeps_them() {
    let (sandbox, _) = task_sandbox();
    let missing = "012345abcdef".repeat(5) + "0123";
    let output = sandbox.command().args(["show", &missing]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        output.stderr,
        b"error: No task matches 012345\n  input <TASK>\nhint: Run `ppn ls` to list tasks.\n"
    );
    let output = sandbox
        .command()
        .args(["show", &missing, "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["task"], missing);
}

#[test]
fn write_diagnostics_keep_one_prefix_and_short_edge_endpoints() {
    let (sandbox, _) = task_sandbox();
    let id = create_task(&sandbox, "Paint the shed", &[]);
    let output = sandbox
        .command()
        .args(["link", &id, "blocks", &id])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "error: link would create a cycle through {}\n  input <TO>\n",
            &id[..6]
        )
    );
    for (args, message) in [
        (
            &["edit", &id, "--title", "   "][..],
            "error: invalid title \"   \": expected a non-empty title\n  input --title\n",
        ),
        (
            &["tag", &id, "Hardware"],
            "error: invalid tag \"Hardware\": expected a lowercase tag of at most 64 bytes\n  input <TAG>\nhint: Use a tag matching [a-z][a-z0-9_./-]* with at most 64 bytes.\n",
        ),
    ] {
        let output = sandbox.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, message.as_bytes());
    }

    sandbox.run(&["view", "save", "weekend", "tag:hardware"]);
    sandbox.run(&["view", "save", "maintenance", "@weekend"]);
    let output = sandbox
        .command()
        .args(["view", "rm", "weekend"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stderr, b"error: view weekend is referenced by maintenance\n  input <NAME>\nhint: Pass `--force` to remove the referenced view.\n");
}

#[test]
fn show_forces_color_on_a_pipe_when_requested_by_flag_or_config() {
    let (sandbox, _) = task_sandbox();
    let id = create_task(&sandbox, "Replace the greenhouse latch", &[]);
    assert!(!sandbox.run(&["show", &id]).contains('\x1b'));
    let show = |flags: &[&str]| {
        let output = sandbox
            .command()
            .env("NO_COLOR", "1")
            .args(["show", &id])
            .args(flags)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        output.stdout
    };
    let forced = show(&["--color", "always"]);
    assert!(forced.contains(&0x1b));
    std::fs::write(
        &sandbox.config,
        "data_dir = '~/board'\n[ui]\ncolor = 'always'\ndate_format = '%d %b %Y'\n",
    )
    .unwrap();
    assert_eq!(show(&[]), forced);
    assert!(!show(&["--color", "auto"]).contains(&0x1b));
    assert_eq!(
        as_json(&sandbox.run(&["show", &id, "--json"]))["kind"],
        "task"
    );
}

#[test]
fn human_diagnostics_use_cli_names_across_commands() {
    let (sandbox, data) = task_sandbox();
    let task = create_task(&sandbox, "Repair the greenhouse latch", &[]);
    let (ambiguous, _) = common::ambiguous_tasks(&data);
    let unusable = "1".repeat(64);
    sandbox.run(&["view", "save", "weekend", "tag:hardware"]);
    sandbox.run(&["view", "save", "maintenance", "@weekend"]);
    let cyclic = format!("parent:{ambiguous}");
    for (args, status) in [
        (&["ls", "--sort", "missing"][..], 2),
        (&["ls", "--filter", "due:<"], 2),
        (&["ls", "@missing"], 2),
        (&["ls", "assignee:unknown"], 2),
        (&["ls", "board:missing"], 3),
        (&["ls", &cyclic], 4),
        (&["ls", "tag:hardware tag:outdoors"], 2),
        (&["view", "save", "Weekend", "tag:hardware"], 2),
        (&["view", "save", "weekend", "@maintenance"], 2),
        (&["view", "rm", "weekend"], 1),
        (&["view", "show", "absent"], 3),
        (&["show", &unusable], 3),
        (&["tree", &ambiguous], 4),
        (&["deps", "task"], 2),
        (&["log", &unusable], 3),
        (&["add", "   "], 2),
        (&["add", "Paint the frame", "--parent", &unusable], 3),
        (&["add", "Paint the frame", "--set", "Invalid=4"], 2),
        (&["edit", &task, "--title", "   "], 2),
        (&["tag", &task, "Hardware"], 2),
        (&["assign", &task, "Unknown Person"], 2),
        (&["comment", &task, "   "], 2),
        (&["link", &task, "blocks", &task], 1),
        (&["unlink", &task, "blocks", &unusable], 3),
        (&["rm", &task], 1),
    ] {
        let output = sandbox.command().args(args).output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(status), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}: {stderr}");
        for identifier in [
            "list_views",
            "at name",
            "at filter",
            "changes.title",
            "field:\"filter\"",
        ] {
            assert!(!stderr.contains(identifier), "{args:?}: {stderr}");
        }
        assert!(
            !stderr
                .split(|ch: char| !ch.is_ascii_hexdigit())
                .any(|word| word.len() == 64),
            "{args:?}: {stderr}"
        );
    }
}
