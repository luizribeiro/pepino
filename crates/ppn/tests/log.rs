mod common;

use common::{
    SEED_NOW, as_json, create_task, seed_tasks, small_project, success, task_error, task_sandbox,
};

#[test]
fn log_renders_task_and_board_history_with_versioned_json() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    let task = create_task(&sandbox, "Hang the greenhouse thermometer", &[]);
    add_comments(&data, &task, 1);

    let json = as_json(&sandbox.run(&["log", &task, "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "log");
    assert_eq!(json["unordered"], serde_json::json!([]));
    let events = json["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["id"], task);
    assert_eq!(events[1]["change"]["action"], "commented");

    let stamps: Vec<_> = events
        .iter()
        .map(|event| {
            let clock = chrono::DateTime::parse_from_rfc3339(event["hlc"]["utc"].as_str().unwrap())
                .unwrap();
            let name = json["names"][event["author"].as_str().unwrap()]
                .as_str()
                .unwrap();
            format!("{}  {name}", clock.format("%d %b %Y %T"))
        })
        .collect();
    let human = success(
        sandbox
            .command()
            .env("TZ", "UTC")
            .args(["log", &task])
            .output()
            .unwrap(),
    );
    assert_eq!(
        human,
        format!(
            "{}  Created Hang the greenhouse thermometer\n{}  Commented “Thermometer reading 0”\n",
            stamps[0], stamps[1]
        )
    );

    let board = as_json(&sandbox.run(&["log", "--json"]));
    assert_eq!(board["kind"], "log");
    assert_eq!(board["events"][0]["id"], project.parent);
    assert_eq!(
        board["events"].as_array().unwrap().last().unwrap(),
        &events[1]
    );
    let output = sandbox
        .command()
        .args(["log", &"0".repeat(64), "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["field"], "task");
}

fn add_comments(data: &std::path::Path, task: &str, count: usize) {
    seed_tasks(data, async |app| {
        for n in 0..count {
            app.handle(
                pepino_app::Request::Write(pepino_app::WriteRequest::Comment {
                    task: task.into(),
                    text: format!("Thermometer reading {n}"),
                }),
                SEED_NOW,
            )
            .await
            .unwrap();
        }
    });
}

#[test]
fn log_selects_the_newest_fifty_events_by_default() {
    let (sandbox, data) = task_sandbox();
    let project = small_project(&sandbox);
    add_comments(&data, &project.parent, 51);

    let json = as_json(&sandbox.run(&["log", &project.parent, "--json"]));
    let rows = json["events"].as_array().unwrap();
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[0]["change"]["text"], "Thermometer reading 1");
    assert_eq!(rows[49]["change"]["text"], "Thermometer reading 50");

    let limited = as_json(&sandbox.run(&["log", "--limit", "1", "--json"]));
    assert_eq!(limited["events"], serde_json::json!([rows[49]]));
    let all = as_json(&sandbox.run(&["log", &project.parent, "--all", "--limit", "1", "--json"]));
    assert_eq!(all["events"][0]["id"], project.parent);
    assert_eq!(all["events"][3]["change"]["text"], "Thermometer reading 0");
    assert_eq!(all["events"].as_array().unwrap().last().unwrap(), &rows[49]);

    assert_eq!(sandbox.run(&["log", "--limit", "0"]), "");
    let output = sandbox
        .command()
        .args(["log", "--limit", "lots", "--json"])
        .output()
        .unwrap();
    task_error(output, 2, "usage");
}
