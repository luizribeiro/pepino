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
    let code = success(output);
    assert!(code.starts_with("PPN-REQUEST-"));
    assert_eq!(sandbox.run(&["device", "request"]), code);
    let json = as_json(&sandbox.run(&["device", "request", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["request"], code.trim());
    assert_eq!(json["secret"], true);
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
    assert!(devices[0]["last_seen"].is_null());
    assert!(
        sandbox
            .run(&["device", "ls"])
            .contains("joined · this device")
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
