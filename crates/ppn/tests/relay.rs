mod common;
use common::{ConfigSandbox, as_json, success, task_error};

#[test]
fn relay_role_is_explicit_and_exclusive_without_a_terminal() {
    let sandbox = ConfigSandbox::new();
    let run = |args: &[&str]| sandbox.command().args(args).output().unwrap();
    task_error(run(&["relay", "show", "--json"]), 5, "permission_denied");
    let info = as_json(&success(run(&["relay", "init", "--json"])));
    assert_eq!(info["schema"], 1);
    let node = info["node_id"].as_str().unwrap();
    assert_eq!(node.len(), 64);
    let human = success(run(&["relay", "init"]));
    assert!(human.contains(node));
    assert_eq!(as_json(&success(run(&["relay", "show", "--json"]))), info);
    assert!(success(run(&["relay", "show"])).contains(node));
    assert!(success(run(&["status"])).contains("Offline"));
    assert_eq!(
        as_json(&success(run(&["status", "--json"])))["role"],
        "relay"
    );
    for arguments in [
        &["ls"][..],
        &["add", "Replace glazing"],
        &["init"],
        &["device", "request"],
    ] {
        let error = run(arguments);
        assert_eq!(error.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&error.stderr).contains("data directory is a relay"));
        let args = [arguments, &["--json"]].concat();
        let error = task_error(run(&args), 5, "permission_denied");
        assert!(error["message"].as_str().unwrap().contains("Converting"));
    }
    let device = ConfigSandbox::new();
    success(device.command().args(["init"]).output().unwrap());
    task_error(
        device
            .command()
            .args(["relay", "init", "--json"])
            .output()
            .unwrap(),
        5,
        "permission_denied",
    );
}
