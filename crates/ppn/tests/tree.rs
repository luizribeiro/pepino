mod common;

use common::{as_json, create_task, small_project, task_error, task_sandbox};

#[test]
fn tree_shows_a_forest_and_omits_closed_tasks_by_default() {
    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let closed = create_task(
        &sandbox,
        "Unpack the fittings",
        &["--parent", &project.blocker, "--status", "done"],
    );
    let leaf = create_task(&sandbox, "Count the spare washers", &["--parent", &closed]);
    let root = create_task(&sandbox, "Prune the apple trees", &[]);
    let expected = format!(
        "{} todo  Renovate the greenhouse  #garden\n  {} todo  Order stainless fittings  #hardware\n    {} todo  Count the spare washers\n  {} todo  Test the new latch  blocked\n{} todo  Prune the apple trees\n",
        &project.parent[..6],
        &project.blocker[..6],
        &leaf[..6],
        &project.downstream[..6],
        &root[..6],
    );
    assert_eq!(sandbox.run(&["tree"]), expected);

    let json = as_json(&sandbox.run(&["tree", &project.blocker, "--all", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "tree");
    assert_eq!(json["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(json["nodes"][0]["depth"], 0);
    assert_eq!(json["nodes"][0]["task"]["parent"], project.parent);
    assert_eq!(
        json["nodes"][0]["task"]["blocks"],
        serde_json::json!([project.downstream])
    );
    assert_eq!(
        json["nodes"][0]["task"]["tags"],
        serde_json::json!(["hardware"])
    );
    assert_eq!(json["nodes"][1]["task"]["id"], closed);
    assert_eq!(json["nodes"][2]["depth"], 2);
    assert_eq!(json["nodes"][2]["task"]["id"], leaf);
    let missing = "0".repeat(64);
    let output = sandbox
        .command()
        .args(["tree", &missing, "--json"])
        .output()
        .unwrap();
    assert_eq!(task_error(output, 3, "not_found")["task"], missing);
}

#[cfg(unix)]
#[test]
fn tree_draws_guides_and_budgets_their_width_on_a_terminal() {
    use nix::pty::{Winsize, openpty};
    use std::{fs::File, io::Read, process::Stdio};

    let (sandbox, _) = task_sandbox();
    let project = small_project(&sandbox);
    let title = "Check the replacement fasteners against the greenhouse plans";
    let leaf = create_task(&sandbox, title, &["--parent", &project.blocker]);
    let pty = openpty(
        Some(&Winsize {
            ws_row: 24,
            ws_col: 52,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let child = sandbox
        .command()
        .args(["tree", "--color", "never"])
        .stdout(File::from(pty.slave))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut bytes = Vec::new();
    if let Err(error) = File::from(pty.master).read_to_end(&mut bytes) {
        assert_eq!(error.raw_os_error(), Some(nix::errno::Errno::EIO as i32));
    }
    common::success(child.wait_with_output().unwrap());

    let tty = String::from_utf8(bytes).unwrap();
    let rows: Vec<_> = tty.lines().collect();
    assert_eq!(rows.len(), 4);
    assert!(rows[1].starts_with(&format!("├── {}", &project.blocker[..6])));
    assert!(rows[2].starts_with(&format!("│   └── {}", &leaf[..6])));
    assert!(rows[3].starts_with(&format!("└── {}", &project.downstream[..6])));
    assert_eq!(rows[2].chars().count(), 52);
    assert!(rows[2].contains('…'));
    assert!(
        sandbox
            .run(&["tree"])
            .contains(&format!("    {} todo  {title}", &leaf[..6]))
    );
}
