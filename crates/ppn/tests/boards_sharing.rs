#![cfg(pepino_network_tests)]

mod common;
#[allow(dead_code)]
#[path = "device/support.rs"]
mod support;
#[path = "common/wait.rs"]
mod wait;

use serde_json::Value;
use std::{
    io::Write,
    process::{Output, Stdio},
};
use support::Host;
use wait::{WAIT, until};

fn host(network: &str, name: &str, device: &str, private: &str) -> Host {
    let host = Host::new(network);
    host.run(&[
        "init", "--name", name, "--device", device, "--board", private,
    ]);
    host.start();
    host
}

fn with_code(host: &Host, args: &[&str], code: &str) -> Output {
    let mut child = host
        .command()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn grant(owner: &Host, board: &str, role: &str, request: &Value) -> Value {
    let code = request["request"].as_str().unwrap();
    let fingerprint = request["fingerprint"].as_str().unwrap();
    common::as_json(&common::success(with_code(
        owner,
        &[
            "board",
            "invite",
            "-",
            "--board",
            board,
            "--role",
            role,
            "--fingerprint",
            fingerprint,
            "--json",
        ],
        code,
    )))
}

fn board(host: &Host, name: &str) -> Value {
    let list = host.json(&["board", "ls"]);
    list["boards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|board| board["name"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn two_identities_share_one_board_and_edit_each_others_work() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let luiz = host(network, "Luiz", "Desktop", "Luiz private");
    let ana = host(network, "Ana", "Tablet", "Ana private");
    let house = luiz.json(&["board", "new", "House"]);
    let task = luiz.json(&["add", "--board", "House", "Repair the window"]);
    let task = task["created"].as_str().unwrap();
    ana.json(&["board", "new", "Ana private notes"]);
    ana.json(&[
        "add",
        "--board",
        "Ana private notes",
        "Private notes on the balcony",
    ]);
    let plain_request = ana.output(&["board", "request", "--name", "Garden tablet"]);
    assert!(plain_request.status.success());
    assert!(String::from_utf8_lossy(&plain_request.stderr).contains("fingerprint"));
    let request = ana.json(&["board", "request", "--name", "Garden tablet"]);
    assert_eq!(
        String::from_utf8_lossy(&plain_request.stdout).trim(),
        request["request"].as_str().unwrap()
    );
    common::task_error(
        with_code(
            &luiz,
            &["board", "invite", "-", "--board", "House", "--json"],
            request["request"].as_str().unwrap(),
        ),
        1,
        "confirmation_required",
    );
    assert_eq!(request["secret"], true);
    let issued = grant(&luiz, "House", "write", &request);
    assert_eq!(issued["grant"]["secret"], true);
    let joined = with_code(
        &ana,
        &["board", "join", "-", "--timeout", "60s", "--json"],
        issued["grant"]["code"].as_str().unwrap(),
    );
    assert_eq!(common::as_json(&common::success(joined))["joined"], true);
    assert_eq!(board(&ana, "House")["id"], house["board"]["id"]);
    assert_eq!(board(&ana, "House")["shared"], true);
    assert!(luiz.run(&["board", "ls"]).contains("shared"));
    assert!(board(&ana, "Luiz private").is_null());
    assert!(board(&luiz, "Ana private").is_null());
    assert!(board(&luiz, "Ana private notes").is_null());
    let owner_note = luiz.json(&["status"]);
    let guest_note = ana.json(&["status"]);
    for (status, role) in [(&owner_note, "manage"), (&guest_note, "write")] {
        assert!(
            status["boards"]
                .as_array()
                .unwrap()
                .iter()
                .any(|board| board["name"] == "House" && board["role"] == role)
        );
    }
    let person = ana.json(&["identity", "show"])["identity"]["id"].clone();
    luiz.json(&["assign", "--board", "House", task, "Ana"]);
    ana.json(&["comment", "--board", "House", task, "New glass arrived"]);
    ana.json(&[
        "edit",
        "--board",
        "House",
        task,
        "--title",
        "Install new window glass",
    ]);
    let second = ana.json(&["add", "--board", "House", "Order window putty"]);
    let second = second["created"].as_str().unwrap();
    ana.json(&["assign", "--board", "House", second, "Luiz"]);
    let confirmed = until(
        || luiz.json(&["ls", "--board", "House", "assignee:me"]),
        |listing| listing["total"] == 1,
        WAIT,
    );
    assert_eq!(confirmed["tasks"][0]["id"], second);
    let show = |host: &Host| host.json(&["show", "--board", "House", task]);
    let result = until(
        || show(&luiz),
        |value| {
            value["task"]["title"] == "Install new window glass"
                && value["task"]["comments"].as_array().unwrap().len() == 1
        },
        WAIT,
    );
    until(|| show(&ana), |value| value == &result, WAIT);
    assert!(
        luiz.run(&["show", "--board", "House", task])
            .contains("Ana")
    );
    assert_eq!(result["assignee_names"][person.as_str().unwrap()], "Ana");
    assert!(
        ana.run(&["board", "members", "House"])
            .contains("Ana (write)")
    );
    ana.run(&["identity", "rename", "Ana Workshop"]);
    until(
        || luiz.json(&["board", "members", "House"]),
        |result| {
            result["people"]
                .as_array()
                .unwrap()
                .iter()
                .any(|person| person["name"] == "Ana Workshop")
        },
        WAIT,
    );
    let people = ana.json(&["board", "members", "House"]);
    let invitee = people["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|person| person["name"] == "Ana Workshop")
        .unwrap();
    assert_eq!(invitee["role"], "write");
    assert_eq!(invitee["invited_by"]["name"], "Luiz");
    assert!(chrono::DateTime::parse_from_rfc3339(invitee["invited_at"].as_str().unwrap()).is_ok());
    common::task_error(
        ana.output(&["board", "members", "Missing", "--json"]),
        3,
        "not_found",
    );
}
