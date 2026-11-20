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
        "Tasks: 0 open, 0 closed",
        "Events: 0 total, 0 applied",
    ] {
        assert!(human.contains(label), "{human}");
    }
    assert!(human.contains(&format!("Data directory: {}", data.display())));
    let status = as_json(&sandbox.run(&["status", "--json"]));
    assert_eq!(
        status,
        json!({
            "schema":1, "kind":"status", "identity": status["identity"], "device": status["device"],
            "board":{"id": status["board"]["id"], "name":"Personal"}, "data_dir":data,
            "counts":{"open":0,"closed":0,"events":0,"applied":0,"skipped":{
                "unknown_version":0,"unknown_event":0,"unknown_value":0,"invalid":0
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
