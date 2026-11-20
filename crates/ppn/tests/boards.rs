mod common;
#[allow(dead_code)]
mod daemon_support;
#[allow(dead_code)]
#[path = "common/wait.rs"]
mod wait;

use common::{as_json, seed_tasks, success, task_error, task_sandbox};
use pepino_core::BoardId;

#[test]
fn selects_a_board_by_flag_env_or_host_current() {
    let (sandbox, data) = task_sandbox();
    let workshop = BoardId::from_bytes([81; 32]);
    seed_tasks(&data, async |app| {
        app.register_created_board(workshop, "Workshop")
            .await
            .unwrap();
    });
    let run = |args: &[&str], value: &str| {
        sandbox
            .command()
            .args(args)
            .env("PPN_BOARD", value)
            .output()
            .unwrap()
    };
    for (label, reference, source) in [
        ("Personal", "", "current"),
        ("Workshop", "Workshop", "PPN_BOARD"),
        ("Workshop", "5151", "PPN_BOARD"),
    ] {
        for args in [&["board", "ls"][..], &["board", "show"], &["status"]] {
            let reply = as_json(&success(run(&[args, &["--json"]].concat(), reference)));
            assert_eq!(reply["selected_board"]["name"], label);
            assert_eq!(reply["selected_board"]["source"], source);
            assert!(success(run(args, reference)).contains(&format!("via {source}")));
        }
    }
    let reply = as_json(&success(run(
        &["--board", "Personal", "board", "show", "--json"],
        "missing",
    )));
    assert_eq!(reply["selected_board"]["source"], "--board");
    assert_eq!(reply["board"]["name"], "Personal");

    let output = run(&["board", "use", "5151", "--json"], "Personal");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["board"]["current"],
        true
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("PPN_BOARD"));
    assert_eq!(
        as_json(&success(run(&["status", "--json"], "")))["board"]["name"],
        "Workshop"
    );
    assert_eq!(
        as_json(&success(run(&["status", "--json"], "Personal")))["board"]["name"],
        "Personal"
    );
    assert!(success(run(&["board", "use", "Personal"], "")).contains("* Personal"));
}

#[test]
fn invalid_and_ambiguous_boards_report_the_input_source() {
    let (sandbox, data) = task_sandbox();
    seed_tasks(&data, async |app| {
        for raw in [51, 61] {
            app.register_created_board(BoardId::from_bytes([raw; 32]), "Workshop")
                .await
                .unwrap();
        }
    });
    for command in [
        &["ls"][..],
        &["board", "ls"],
        &["board", "show"],
        &["status"],
        &["add", "Sand boards"],
        &["show", "1234"],
    ] {
        for (value, status, code) in [("missing", 3, "not_found"), ("Workshop", 4, "ambiguous_id")]
        {
            let output = sandbox
                .command()
                .args(command)
                .arg("--json")
                .env("PPN_BOARD", value)
                .output()
                .unwrap();
            let error = task_error(output, status, code);
            assert!(error["message"].as_str().unwrap().contains("PPN_BOARD"));
            if status == 4 {
                assert_eq!(error["candidates"].as_array().unwrap().len(), 2);
            }
        }
    }
    for command in [
        &["board", "show", "Workshop"][..],
        &["board", "use", "Workshop"],
        &["board", "rename", "Workshop", "Tool Shed"],
    ] {
        let json = sandbox
            .command()
            .args(command)
            .arg("--json")
            .output()
            .unwrap();
        let error = task_error(json, 4, "ambiguous_id");
        assert_eq!(error["candidates"].as_array().unwrap().len(), 2);
        let human = sandbox.command().args(command).output().unwrap();
        assert_eq!(human.status.code(), Some(4));
        assert!(String::from_utf8_lossy(&human.stderr).contains("ambiguous"));
    }
    task_error(
        sandbox
            .command()
            .args(["board", "rename", "Personal", " ", "--json"])
            .output()
            .unwrap(),
        2,
        "invalid_input",
    );
    for command in [
        &["board", "show", "missing"][..],
        &["board", "use", "missing"],
        &["board", "rename", "missing", "Tool Shed"],
    ] {
        task_error(
            sandbox
                .command()
                .args(command)
                .arg("--json")
                .output()
                .unwrap(),
            3,
            "not_found",
        );
        let human = sandbox.command().args(command).output().unwrap();
        assert_eq!(human.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&human.stderr).contains("missing"));
    }
}

#[test]
fn creates_through_the_daemon_and_renders_renames() {
    let fixture = daemon_support::Fixture::new("");
    let command = |args: &[&str]| fixture.command().args(args).output().unwrap();
    task_error(
        command(&["board", "new", "Workshop", "--json"]),
        6,
        "daemon_unreachable",
    );
    task_error(
        command(&["board", "new", " ", "--json"]),
        2,
        "invalid_input",
    );
    let created = as_json(&success(
        fixture
            .command()
            .env_remove("PPN_NO_DAEMON")
            .args(["board", "new", "Workshop", "--json"])
            .output()
            .unwrap(),
    ));
    assert_eq!(created["board"]["name"], "Workshop");
    assert_eq!(created["board"]["membership"], "joined");
    let pid = fixture.ready()["pid"].clone();
    assert!(success(command(&["board", "new", "Workshop"])).contains("Workshop"));
    let board = created["board"]["id"].as_str().unwrap();
    let renamed = as_json(&success(command(&[
        "board",
        "rename",
        &board[..8],
        "Tool Shed",
        "--json",
    ])));
    assert_eq!(renamed["board"]["id"], board);
    assert!(success(command(&["board", "rename", "Tool Shed", "Wood Shop"])).contains("Wood Shop"));
    assert_eq!(fixture.ready()["pid"], pid);
}

#[test]
fn task_commands_use_one_board_and_listings_can_span_boards() {
    let (sandbox, data) = task_sandbox();
    seed_tasks(&data, async |app| {
        app.register_created_board(BoardId::from_bytes([81; 32]), "Workshop")
            .await
            .unwrap();
    });
    let local = as_json(&sandbox.run(&["add", "Water the fig tree", "--json"]));
    let remote = as_json(&success(
        sandbox
            .command()
            .env("PPN_BOARD", "5151")
            .args(["add", "Sand the workbench", "--json"])
            .output()
            .unwrap(),
    ));
    let local = local["created"].as_str().unwrap();
    let remote = remote["created"].as_str().unwrap();
    assert!(sandbox.run(&["ls"]).contains("Water the fig tree"));
    assert!(!sandbox.run(&["ls"]).contains("Sand the workbench"));
    assert!(
        sandbox
            .run(&["--board", "Workshop", "ls"])
            .contains("Sand the workbench")
    );
    assert!(
        sandbox
            .run(&["ls", "--all-boards"])
            .contains("Workshop (515151)")
    );
    let both = as_json(&sandbox.run(&["ls", "--all-boards", "--sort", "title", "--json"]));
    assert_eq!(both["tasks"][0]["board"]["name"], "Workshop");
    assert_eq!(both["tasks"][1]["board"]["name"], "Personal");
    let filtered = as_json(&sandbox.run(&["ls", "board:5151", "--json"]));
    assert_eq!(filtered["tasks"][0]["id"], remote);
    let missing = task_error(
        sandbox
            .command()
            .args(["show", &remote[..8], "--json"])
            .output()
            .unwrap(),
        3,
        "not_found",
    );
    assert!(
        missing["hint"]
            .as_str()
            .unwrap()
            .contains("Workshop (515151)")
    );
    for args in [
        vec!["link", local, "blocks", remote],
        vec!["unlink", local, "blocks", remote],
        vec!["add", "Build the shelves", "--parent", remote],
    ] {
        let error = task_error(
            sandbox.command().args(args).arg("--json").output().unwrap(),
            2,
            "invalid_input",
        );
        assert_eq!(error["message"], "both tasks must be on the same board");
    }
}
