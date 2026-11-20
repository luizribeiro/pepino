mod common;
use common::*;

#[test]
fn init_names_the_local_records_and_is_idempotent() {
    let (sandbox, data) = uninitialized_task_sandbox();
    let output = sandbox.run(&[
        "init",
        "--name",
        "Ada Gardener",
        "--device",
        "Garden laptop",
        "--board",
        "Greenhouse",
    ]);
    assert!(output.contains("Identity: Ada Gardener"));
    assert!(output.contains("Device: Garden laptop"));
    assert!(output.contains("Board: Greenhouse"));
    let before = std::fs::read(data.join("local.json")).unwrap();
    let status = sandbox.run(&["status", "--json"]);
    let repeated = sandbox.run(&[
        "init",
        "--name",
        "Another name",
        "--board",
        "Another board",
        "--json",
    ]);
    assert_eq!(as_json(&status), as_json(&repeated));
    assert_eq!(std::fs::read(data.join("local.json")).unwrap(), before);
}

#[test]
fn task_commands_require_init_and_do_not_create_local_state() {
    let (sandbox, data) = uninitialized_task_sandbox();
    for args in [
        &["ls", "--json"][..],
        &["add", "Order hinges", "--json"],
        &["status", "--json"],
    ] {
        let error = task_error(
            sandbox.command().args(args).output().unwrap(),
            1,
            "not_initialized",
        );
        assert_eq!(error["hint"], "Run `ppn init`.");
        assert!(!data.exists());
    }
    sandbox.run(&["init"]);
    let state = data.join("local.json");
    std::fs::remove_file(&state).unwrap();
    for command in ["ls", "init"] {
        let error = task_error(
            sandbox
                .command()
                .args([command, "--json"])
                .output()
                .unwrap(),
            1,
            "failure",
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("pepino.sqlite exists without")
        );
        assert!(!state.exists());
    }
}
