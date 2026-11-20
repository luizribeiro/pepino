use serde_json::Value;

use crate::{
    common::task_error,
    support::Host,
    wait::{WAIT, until},
};

fn link(owner: &Host, device: &Host, name: &str) {
    let grant = owner.add(&device.request(name), &[]);
    use std::{io::Write, process::Stdio};
    let mut child = device
        .command()
        .args(["device", "join", "-", "--timeout", "20s", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(grant.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{output:?}\nDevice status: {}\nGrantor daemon: {}",
        device.json(&["status"]),
        owner.json(&["daemon", "status"])
    );
}

fn id(host: &Host) -> String {
    host.json(&["status"])["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn roster_state(host: &Host, device: &str) -> Option<Value> {
    host.json(&["device", "ls"])["roster"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == device)
        .cloned()
}

fn check_task(host: &Host, title: &str) -> bool {
    host.json(&["ls", "--all-boards"])["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|task| task["title"] == title)
}

#[test]
fn online_revocation_rotates_access_across_boards_and_preserves_history() {
    let directory = tempfile::tempdir().unwrap();
    let network = directory.path().file_name().unwrap().to_str().unwrap();
    let owner = Host::owner(network);
    owner.run(&["board", "new", "Orchard"]);
    let peer = Host::new(network);
    let tablet = Host::new(network);
    link(&owner, &peer, "Ferrum");
    link(&owner, &tablet, "Garden tablet");
    let target = id(&tablet);
    tablet.run(&["add", "Sharpen the pruning shears", "--board", "Orchard"]);
    tablet.run(&["sync", "--timeout", "30s"]);
    for host in [&owner, &peer] {
        until(
            || check_task(host, "Sharpen the pruning shears"),
            |visible| *visible,
            WAIT,
        );
    }
    let result = owner.json(&["device", "revoke", &target, "--yes"]);
    let boards = result["boards"].as_array().unwrap();
    assert_eq!(boards.len(), 2, "{result}");
    assert!(
        boards.iter().all(|board| board["result"] == "removed"),
        "{result}"
    );
    until(
        || roster_state(&peer, &target),
        |row| row.as_ref().is_some_and(|row| row["state"] == "revoked"),
        WAIT,
    );
    until(
        || tablet.json(&["status"])["device"]["revocation"].clone(),
        Value::is_object,
        WAIT,
    );
    let error = task_error(
        tablet.output(&["add", "Retired draft", "--json"]),
        5,
        "permission_denied",
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("revoked from identity Ada Gardener")
    );
    for host in [&owner, &peer, &tablet] {
        assert!(check_task(host, "Sharpen the pruning shears"));
    }
    for title in ["Measure the tool rack", "Label the apple trees"] {
        let board = if title.contains("apple") {
            "Orchard"
        } else {
            "Greenhouse"
        };
        owner.run(&["add", title, "--board", board]);
        until(|| check_task(&peer, title), |visible| *visible, WAIT);
        assert!(!check_task(&tablet, title));
    }
}
