mod common;
#[allow(dead_code)]
mod daemon_support;

use common::{as_json, seed_tasks, success, task_error, task_sandbox};
use pepino_core::BoardId;

#[test]
fn reports_a_named_board_in_human_and_json_output() {
    let (sandbox, data) = task_sandbox();
    seed_tasks(&data, async |app| {
        app.register_created_board(BoardId::from_bytes([81; 32]), "Tool Shed")
            .await
            .unwrap();
    });
    assert!(sandbox.run(&["board", "ls"]).contains("Tool Shed (515151)"));
    let board = as_json(&sandbox.run(&["board", "show", "5151", "--json"]));
    assert_eq!(board["board"]["name"], "Tool Shed");
    assert_eq!(board["selected_board"]["source"], "current");
}

#[test]
fn renames_and_selects_a_board_locally() {
    let (sandbox, data) = task_sandbox();
    seed_tasks(&data, async |app| {
        app.register_created_board(BoardId::from_bytes([81; 32]), "Tool Shed")
            .await
            .unwrap();
    });
    assert!(
        sandbox
            .run(&["board", "use", "5151"])
            .contains("* Tool Shed")
    );
    let response = as_json(&sandbox.run(&["board", "rename", "Tool Shed", "Workshop", "--json"]));
    assert_eq!(response["board"]["name"], "Workshop");
    assert_eq!(response["board"]["current"], true);
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
