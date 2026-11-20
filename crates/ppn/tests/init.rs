mod common;
use common::*;

#[test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
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
        &["edit", "deadbeef", "--json"],
        &[
            "comment",
            "deadbeef",
            "--from-file",
            "missing-comment.txt",
            "--json",
        ],
    ] {
        let error = task_error(
            sandbox.command().args(args).output().unwrap(),
            1,
            "not_initialized",
        );
        assert_eq!(error["hint"], "Run `ppn init`.");
        assert!(!data.exists());
    }
    fixture::copy(&data);
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

#[cfg(unix)]
#[test]
fn init_reports_a_read_only_data_directory_as_permission_denied() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let (sandbox, data) = uninitialized_task_sandbox();
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::write(data.join("probe"), "").is_ok() {
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let output = sandbox.command().args(["init", "--json"]).output().unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let error = task_error(output, 5, "permission_denied");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(data.to_str().unwrap())
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_config_file_reports_permission_denied_and_its_path() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let (sandbox, data) = uninitialized_task_sandbox();
    fs::set_permissions(&sandbox.config, fs::Permissions::from_mode(0o000)).unwrap();
    let can_read = fs::read(&sandbox.config).is_ok();
    let output = sandbox.command().args(["ls", "--json"]).output().unwrap();
    fs::set_permissions(&sandbox.config, fs::Permissions::from_mode(0o600)).unwrap();
    if !can_read {
        let error = task_error(output, 5, "permission_denied");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains(sandbox.config.to_str().unwrap())
        );
        assert!(!data.exists());
    }
}
