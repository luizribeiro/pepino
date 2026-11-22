mod common;
use common::*;

#[test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
fn init_names_the_local_records_and_is_idempotent() {
    use pepino_node::DeviceCredentials;
    use std::fs;
    let (sandbox, data) = uninitialized_task_sandbox();
    let credential_path = sandbox.home.path().join("keys/device.cbor");
    let mut config = fs::read_to_string(&sandbox.config).unwrap();
    config.insert_str(0, "credentials_file = '~/keys/device.cbor'\n");
    fs::write(&sandbox.config, config).unwrap();
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
    assert!(output.contains("ppn add"));
    assert_eq!(output.lines().count(), 5);
    assert_eq!(
        sandbox.run(&["init"]).trim(),
        "Pepino is already initialised."
    );
    let before = std::fs::read(data.join("local.json")).unwrap();
    let status = sandbox.run(&["status", "--json"]);
    let document = as_json(&status);
    assert_eq!(
        document["credentials_file"],
        credential_path.to_str().unwrap()
    );
    let device = DeviceCredentials::load_or_create(&credential_path)
        .unwrap()
        .device_id()
        .to_string();
    assert_eq!(document["device"]["id"], device);
    assert!(data.join("node.sqlite").is_file());
    assert!(!data.join("credentials.cbor").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&credential_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let task = create_task(&sandbox, "Replace the greenhouse latch", &[]);
    let task = show_task(&sandbox, &task);
    assert_eq!(task["author"], device);
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
    fixture::prepare_blocking(&data);
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

#[test]
fn old_local_state_is_rejected_without_touching_the_directory() {
    let (sandbox, data) = uninitialized_task_sandbox();
    std::fs::create_dir(&data).unwrap();
    let bytes = br#"{"version":2}"#;
    std::fs::write(data.join("local.json"), bytes).unwrap();
    std::fs::write(data.join("pepino.sqlite"), b"old database").unwrap();
    for command in ["status", "init"] {
        let human = sandbox.command().arg(command).output().unwrap();
        assert_eq!(human.status.code(), Some(1));
        assert!(human.stdout.is_empty());
        let diagnostic = String::from_utf8(human.stderr).unwrap();
        assert!(diagnostic.contains("cannot be upgraded"));
        assert!(diagnostic.contains("Move the directory aside and run `ppn init`."));
        let error = task_error(
            sandbox
                .command()
                .args([command, "--json"])
                .output()
                .unwrap(),
            1,
            "failure",
        );
        assert_eq!(
            error["hint"],
            "Move the directory aside and run `ppn init`."
        );
        assert_eq!(std::fs::read(data.join("local.json")).unwrap(), bytes);
        assert_eq!(
            std::fs::read(data.join("pepino.sqlite")).unwrap(),
            b"old database"
        );
        assert_eq!(std::fs::read_dir(&data).unwrap().count(), 2);
    }
}
