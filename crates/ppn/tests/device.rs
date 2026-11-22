mod common;

use common::*;

#[test]
fn device_request_refuses_to_merge_an_existing_identity() {
    let (sandbox, _) = task_sandbox();
    let args = ["device", "request"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&human.stderr).contains("cannot merge two identities' boards"));
    task_error(
        sandbox.command().args(args).arg("--json").output().unwrap(),
        2,
        "invalid_input",
    );
}

#[cfg(pepino_network_tests)]
#[test]
fn device_request_is_stable_for_a_piped_script() {
    let (sandbox, data) = uninitialized_task_sandbox();
    let args = ["device", "request", "--name", "Ferrum"];
    let output = sandbox.command().args(args).output().unwrap();
    let note = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(note.contains("Device Ferrum, fingerprint"));
    assert!(note.contains("request code is secret"));
    assert!(note.contains("Expires "));
    assert!(output.status.success());
    let code = String::from_utf8(output.stdout).unwrap();
    assert!(code.starts_with("PPN-REQUEST-"));
    let repeated = sandbox
        .command()
        .args(["device", "request"])
        .output()
        .unwrap();
    assert!(repeated.status.success());
    assert_eq!(String::from_utf8(repeated.stdout).unwrap(), code);
    let json = as_json(&sandbox.run(&["device", "request", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["request"], code.trim());
    assert_eq!(json["secret"], true);
    assert!(chrono::DateTime::parse_from_rfc3339(json["expires_at"].as_str().unwrap()).is_ok());
    assert!(note.contains(json["fingerprint"].as_str().unwrap()));
    assert_eq!(json["device_id"].as_str().unwrap().len(), 64);
    assert!(data.join("node.sqlite").exists());
    assert!(!data.join("local.json").exists());
}

#[test]
fn device_add_rejects_a_corrupt_request_without_starting_a_daemon() {
    let (sandbox, data) = uninitialized_task_sandbox();
    let args = ["device", "add", "PPN-REQUEST-ABCDE", "--json"];
    assert_json_error(
        sandbox.command().args(args).output().unwrap(),
        2,
        "invalid_input",
    );
    assert!(!data.exists());
}

#[test]
fn device_join_rejects_invalid_payloads_without_creating_state() {
    use ppn::device_code::Kind;
    let (sandbox, data) = uninitialized_task_sandbox();
    let code = Kind::Grant.encode(&"Ferrum").unwrap();
    let output = sandbox
        .command()
        .args(["device", "join", &code, "--json"])
        .output()
        .unwrap();
    assert_json_error(output, 2, "invalid_input");
    assert!(!data.exists());
}

#[test]
fn device_ls_reports_the_current_device_in_human_and_json_output() {
    let (sandbox, _) = task_sandbox();
    let result = as_json(&sandbox.run(&["device", "ls", "--json"]));
    let devices = result["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["this_device"], true);
    assert_eq!(devices[0]["state"], "joined");
    assert_eq!(devices[0]["member_of"], 1);
    assert_eq!(devices[0]["total_boards"], 1);
    assert_eq!(devices[0]["roster_state"], "active");
    assert_eq!(result["roster"][0]["state"], "active");
    assert_eq!(result["roster"][0]["id"], devices[0]["id"]);
    assert!(devices[0]["last_seen"].is_null());
    assert!(
        sandbox
            .run(&["device", "ls"])
            .contains("active · member of 1 of 1 board · joined · this device")
    );

    let (empty, _) = uninitialized_task_sandbox();
    task_error(
        empty
            .command()
            .args(["device", "ls", "--json"])
            .output()
            .unwrap(),
        1,
        "not_initialized",
    );
}

#[cfg(pepino_network_tests)]
#[path = "device/support.rs"]
mod support;

#[cfg(pepino_network_tests)]
#[path = "common/wait.rs"]
mod wait;

#[cfg(pepino_network_tests)]
#[path = "device/workflow.rs"]
mod workflow;

#[test]
fn private_code_inputs_keep_codes_out_of_the_arguments() {
    use std::{io::Write, process::Stdio};

    let (sandbox, _) = uninitialized_task_sandbox();
    let path = sandbox.home.path().join("secret.txt");
    for (action, code) in [("add", "PPN-REQUEST-ABCDE"), ("join", "PPN-GRANT-ABCDE")] {
        std::fs::write(&path, code).unwrap();
        for source in [
            vec![],
            vec!["-"],
            vec!["--from-file", path.to_str().unwrap()],
        ] {
            let mut command = sandbox.command();
            let reads_stdin = !source.contains(&"--from-file");
            command
                .args(["device", action, "--json"])
                .args(source)
                .stdin(if reads_stdin {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            assert!(command.get_args().all(|arg| arg != code));
            let mut child = command.spawn().unwrap();
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(code.as_bytes()).unwrap();
            }
            let output = child.wait_with_output().unwrap();
            assert_json_error(output, 2, "invalid_input");
        }
    }
}

#[test]
fn cancelling_a_request_removes_pending_state_and_is_idempotent() {
    let (sandbox, data) = uninitialized_task_sandbox();
    std::fs::create_dir_all(&data).unwrap();
    let request = data.join("device-request.json");
    let node = data.join("node.sqlite");
    std::fs::write(&request, "{}").unwrap();
    std::fs::write(&node, "prepared node").unwrap();
    for args in [
        &["init"][..],
        &["status"],
        &["sync"],
        &["ls"],
        &["device", "add"],
        &["daemon", "status"],
    ] {
        let error = task_error(
            sandbox.command().args(args).arg("--json").output().unwrap(),
            1,
            "device_request_pending",
        );
        assert_eq!(
            error["message"],
            "Device request pending: run `ppn device join`"
        );
    }
    let args = ["device", "request", "--cancel"];
    assert_eq!(sandbox.run(&args).trim(), "Device request cancelled.");
    assert!(!request.exists());
    assert!(!node.exists());
    let output = sandbox.run(&["device", "request", "--cancel", "--json"]);
    assert_eq!(as_json(&output)["cancelled"], false);
    task_error(
        sandbox
            .command()
            .args(["status", "--json"])
            .output()
            .unwrap(),
        1,
        "not_initialized",
    );
}

#[test]
fn device_rename_accepts_this_device_or_an_explicit_id_in_a_pipe() {
    let (sandbox, _) = task_sandbox();
    assert!(
        sandbox
            .run(&["device", "rename", " Garden tablet "])
            .contains("Garden tablet")
    );
    let roster = as_json(&sandbox.run(&["device", "ls", "--json"]));
    let id = roster["devices"][0]["id"].as_str().unwrap();
    let changed = as_json(&sandbox.run(&["device", "rename", id, "--json", "Titanium"]));
    assert_eq!(changed["schema"], 1);
    assert_eq!(changed["device"]["id"], id);
    assert_eq!(changed["device"]["name"], "Titanium");
    task_error(
        sandbox
            .command()
            .args(["device", "rename", "missing", "Ferrum", "--json"])
            .output()
            .unwrap(),
        3,
        "not_found",
    );
    task_error(
        sandbox
            .command()
            .args(["device", "rename", " ", "--json"])
            .output()
            .unwrap(),
        2,
        "invalid_input",
    );
}

#[test]
fn revoke_guards_the_current_device_and_confirms_another_device() {
    let (sandbox, data) = uninitialized_task_sandbox();
    sandbox.run(&["init", "--name", "Ada Gardener", "--board", "Greenhouse"]);
    let peer = pepino_core::DeviceId::from_bytes([66; 32]);
    seed_tasks(&data, async |app| {
        app.remember_device(peer, "Garden tablet".into(), None)
            .await
            .unwrap();
    });
    let own = as_json(&sandbox.run(&["status", "--json"]))["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let self_error = sandbox
        .command()
        .args(["device", "revoke", &own, "--yes", "--json"])
        .output()
        .unwrap();
    let self_error = task_error(self_error, 2, "invalid_input");
    assert!(
        self_error["message"]
            .as_str()
            .unwrap()
            .contains("first device anchors the identity")
    );
    let mut command = sandbox.command();
    command.args(["device", "revoke", "Garden tablet"]);
    let declined = task_terminal(command, Some(b"n\n"));
    assert!(declined.status.success());
    assert!(String::from_utf8_lossy(&declined.stdout).contains("Revocation cancelled"));
    let prompt = String::from_utf8_lossy(&declined.stderr);
    for expected in [
        "Garden tablet",
        &peer.to_string()[..6],
        "fingerprint",
        "Boards affected",
        "stale data",
        "full history",
    ] {
        assert!(prompt.contains(expected), "{prompt}");
    }
    assert_json_error(
        sandbox
            .command()
            .args(["device", "revoke", "Garden tablet", "--json"])
            .output()
            .unwrap(),
        1,
        "confirmation_required",
    );
    let result = as_json(&sandbox.run(&["device", "revoke", "Garden tablet", "--yes", "--json"]));
    assert_eq!(result["device_id"], peer.to_string());
    assert_eq!(result["notices"], serde_json::json!([]));
    assert_eq!(result["boards"][0]["result"], "unavailable");
    assert!(
        sandbox
            .run(&["device", "revoke", "Garden tablet", "--yes"])
            .contains("daemon or board unavailable")
    );
    let directory = sandbox.run(&["device", "ls", "--json"]);
    let directory = as_json(&directory);
    for output in [&directory["roster"], &directory["devices"]] {
        let rows = output.as_array().unwrap();
        assert_eq!(
            rows.iter().find(|row| row["id"] == own).unwrap()["first_device"],
            true
        );
        assert_eq!(
            rows.iter()
                .find(|row| row["id"] == peer.to_string())
                .unwrap()["first_device"],
            false
        );
    }
    let identity = as_json(&sandbox.run(&["identity", "show", "--json"]));
    assert_eq!(
        identity["identity"]["roster"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == own)
            .unwrap()["first_device"],
        true,
    );
    assert!(
        sandbox
            .run(&["identity", "show"])
            .contains("(first device)")
    );
    assert!(sandbox.run(&["device", "ls"]).contains(" · first device"));
    let target = directory["roster"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["id"] == peer.to_string())
        .unwrap();
    assert_eq!(target["revocation"]["by"], own);
    assert!(
        target["revocation"]["at"]
            .as_str()
            .is_some_and(|date| chrono::DateTime::parse_from_rfc3339(date).is_ok())
    );
    let active = as_json(&sandbox.run(&["device", "ls", "--active", "--json"]));
    assert_eq!(active["roster"].as_array().unwrap().len(), 1);
    let mut command = sandbox.command();
    command.args(["device", "ls"]);
    let tty = task_terminal(command, None);
    assert!(String::from_utf8_lossy(&tty.stdout).contains("\x1b[2mGarden tablet"));
}

#[test]
fn the_first_device_cannot_leave() {
    let (sandbox, _) = uninitialized_task_sandbox();
    sandbox.run(&["init", "--name", "Ada Gardener"]);
    let output = sandbox
        .command()
        .args(["device", "leave", "--json"])
        .output()
        .unwrap();
    let result = task_error(output, 2, "invalid_input");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("first device anchors the identity")
    );
    let human = sandbox
        .command()
        .args(["device", "leave"])
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&human.stderr).contains("first device anchors the identity"));
}

#[cfg(pepino_network_tests)]
#[path = "device/removal.rs"]
mod removal;

#[test]
fn code_commands_report_the_expected_and_received_kinds_without_echoing_the_input() {
    let (sandbox, _) = common::uninitialized_task_sandbox();
    for (command, action, expected, wrong, received) in [
        (
            "board",
            "join",
            "board grant code",
            "PPN-BOARD-REQUEST",
            "board request code",
        ),
        (
            "device",
            "join",
            "device grant code",
            "PPN-REQUEST",
            "device request code",
        ),
        (
            "device",
            "add",
            "device request code",
            "PPN-BOARD-GRANT",
            "board grant code",
        ),
        (
            "board",
            "invite",
            "board request code",
            "PPN-GRANT",
            "device grant code",
        ),
    ] {
        for (code, given) in [
            (String::new(), "no code"),
            (format!("{wrong}-SECRET"), received),
        ] {
            let human = sandbox
                .command()
                .args([command, action, &code])
                .output()
                .unwrap();
            assert_eq!(human.status.code(), Some(2), "{human:?}");
            assert!(human.stdout.is_empty());
            let diagnostic = String::from_utf8(human.stderr).unwrap();
            let message = format!("Expected a {expected}, received {given}.");
            assert!(diagnostic.contains(&message), "{diagnostic}");
            assert!(!diagnostic.contains("SECRET"));
            let error = common::task_error(
                sandbox
                    .command()
                    .args([command, action, &code, "--json"])
                    .output()
                    .unwrap(),
                2,
                "invalid_input",
            );
            assert_eq!(error["message"], message);
        }
    }
}
