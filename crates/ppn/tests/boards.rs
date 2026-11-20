mod common;

use common::{as_json, seed_tasks, task_sandbox};
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
