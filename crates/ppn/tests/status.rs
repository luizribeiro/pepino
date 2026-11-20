use serde_json::json;

mod common;
use common::*;

#[test]
fn status_renders_empty_local_state_through_the_binary() {
    let (sandbox, data) = task_sandbox();
    let human = sandbox.run(&["status"]);
    for label in [
        "Identity:",
        "Device:",
        "Board: Personal",
        "Personal: joined",
        "Tasks: 0 open, 0 closed",
        "Events: 0 total, 0 applied",
        "network: offline (daemon not running)",
    ] {
        assert!(human.contains(label), "{human}");
    }
    assert!(human.contains(&format!("Data directory: {}", data.display())));
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let id = status["device"]["id"].as_str().unwrap();
    assert!(human.contains(&format!("(node ID {})", &id[..6])));
    assert!(human.contains(&format!(
        "Credentials: {}",
        data.join("credentials.cbor").display()
    )));
    assert_eq!(
        status,
        json!({
            "schema":1, "kind":"status", "identity": status["identity"], "device": status["device"],
            "board":{"id": status["board"]["id"], "name":"Personal"}, "data_dir":data,
            "selected_board":{"id":status["board"]["id"],"name":"Personal","source":"current"},
            "boards":[{"id":status["board"]["id"],"name":"Personal","membership":"joined","sync":"offline"}],
            "credentials_file":data.join("credentials.cbor"), "network":"offline",
            "counts":{"open":0,"closed":0,"events":0,"applied":0,"skipped":{
                "unknown_version":0,"unknown_event":0,"unknown_value":0,"invalid":0,"forged_author":0
            }},
            "protocol_version":1
        })
    );
    for (kind, name) in [
        ("identity", "display_name"),
        ("device", "name"),
        ("board", "name"),
    ] {
        assert_eq!(status[kind]["id"].as_str().unwrap().len(), 64);
        assert!(!status[kind][name].as_str().unwrap().is_empty());
    }
}

#[test]
fn status_honors_zero_timeout_on_the_data_directory_lock() {
    let (sandbox, data) = task_sandbox();
    sandbox.run(&["status"]);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("local.lock"))
        .unwrap();
    lock.lock().unwrap();
    let output = sandbox
        .command()
        .args(["status", "--timeout", "0", "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 1, "busy");
    assert!(error["hint"].is_null());
    lock.unlock().unwrap();
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["kind"],
        "status"
    );
}

#[test]
fn invalid_persisted_state_has_a_cli_diagnostic() {
    let (sandbox, data) = task_sandbox();
    std::fs::write(data.join("local.json"), "{}").unwrap();
    let human = sandbox.command().arg("status").output().unwrap();
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    assert_eq!(
        human.stderr,
        b"error: Could not read the local Pepino state.\n"
    );
    let structured = sandbox
        .command()
        .args(["status", "--json"])
        .output()
        .unwrap();
    let error = task_error(structured, 1, "failure");
    assert!(
        error["reason"]
            .as_str()
            .unwrap()
            .starts_with("decode local state")
    );
}

#[cfg(unix)]
#[test]
fn permission_errors_report_the_path_without_internal_fields() {
    use std::os::unix::fs::PermissionsExt;
    let (sandbox, data) = task_sandbox();
    let file = data.join("local.json");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&file).is_ok() {
        return;
    }
    let args = ["status"];
    let human = sandbox.command().args(args).output().unwrap();
    assert_eq!(human.status.code(), Some(5));
    assert!(human.stdout.is_empty());
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(stderr.contains("permission denied"));
    assert!(stderr.contains(file.to_str().unwrap()));
    let structured = sandbox.command().args(args).arg("--json").output().unwrap();
    assert_eq!(
        task_error(structured, 5, "permission_denied")["path"],
        file.to_str().unwrap()
    );
}
