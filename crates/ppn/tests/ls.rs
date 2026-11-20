mod common;

use common::{as_json, small_project, task_error, task_sandbox};

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
