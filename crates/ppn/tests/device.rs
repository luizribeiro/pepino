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
    let code = sandbox.run(&args);
    assert!(code.starts_with("PPN-REQUEST-"));
    assert_eq!(sandbox.run(&["device", "request"]), code);
    let json = as_json(&sandbox.run(&["device", "request", "--json"]));
    assert_eq!(json["schema"], 1);
    assert_eq!(json["request"], code.trim());
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
