#![cfg(pepino_network_tests)]

mod common;
#[allow(dead_code)]
#[path = "device/support.rs"]
mod support;
#[path = "common/wait.rs"]
mod wait;

use support::Host;
use wait::{WAIT, until};

fn connect(owner: &Host, peer: &Host, name: &str) {
    let request = peer.request(name);
    let grant = owner.add(&request, &[]);
    peer.run(&["device", "join", &grant, "--timeout", "25s"]);
}

fn board_views(host: &Host) -> serde_json::Value {
    host.json(&["board", "ls"])["boards"].clone()
}

fn replicated_board_views(host: &Host) -> serde_json::Value {
    let mut boards = board_views(host);
    for board in boards.as_array_mut().unwrap() {
        board.as_object_mut().unwrap().remove("sync");
    }
    boards
}

fn wait_for_board(host: &Host, name: &str) {
    until(
        || board_views(host),
        |boards| {
            boards
                .as_array()
                .unwrap()
                .iter()
                .any(|board| board["name"] == name && board["membership"] == "joined")
        },
        WAIT,
    );
}

fn sync_hosts(hosts: &[&Host]) {
    for host in hosts {
        assert_eq!(host.json(&["sync", "--timeout", "20s"])["idle"], true);
    }
}

#[test]
fn running_daemons_replicate_creations_renames_and_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let network = directory.path().file_name().unwrap().to_str().unwrap();
    let owner = Host::owner(network);
    let peer = Host::new(network);
    connect(&owner, &peer, "Garden tablet");
    let owner_pid = owner.json(&["daemon", "status"])["pid"].clone();
    let peer_pid = peer.json(&["daemon", "status"])["pid"].clone();
    owner.json(&["board", "new", "Workshop"]);
    let task = owner.json(&["add", "--board", "Workshop", "Hang the spade"]);
    wait_for_board(&peer, "Workshop");
    until(
        || peer.json(&["ls", "board:Workshop"]),
        |listing| listing["total"] == 1,
        WAIT,
    );
    sync_hosts(&[&owner, &peer]);
    let id = task["created"].as_str().unwrap();
    let remote = peer.json(&["show", "--board", "Workshop", id]);
    assert_eq!(remote["task"]["title"], "Hang the spade");
    let owner_board = board_views(&owner)
        .as_array()
        .unwrap()
        .iter()
        .find(|board| board["name"] == "Workshop")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let selected = common::success(
        peer.command()
            .env("PPN_BOARD", &owner_board[..8])
            .args(["show", id, "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(common::as_json(&selected), remote);
    let all = peer.json(&["ls", "--all-boards"]);
    assert_eq!(all["tasks"][0]["board"]["id"], owner_board);
    let missing = common::task_error(peer.output(&["show", &id[..8], "--json"]), 3, "not_found");
    assert!(missing["hint"].as_str().unwrap().contains("Workshop"));

    peer.json(&["board", "rename", "Workshop", "Tool Shed"]);
    wait_for_board(&owner, "Tool Shed");
    sync_hosts(&[&owner, &peer]);

    std::thread::scope(|scope| {
        scope.spawn(|| owner.json(&["board", "new", "Orchard"]));
        scope.spawn(|| peer.json(&["board", "new", "Nursery"]));
    });
    for host in [&owner, &peer] {
        wait_for_board(host, "Orchard");
        wait_for_board(host, "Nursery");
    }
    sync_hosts(&[&owner, &peer]);
    assert_eq!(
        replicated_board_views(&owner),
        replicated_board_views(&peer)
    );
    assert_eq!(owner.json(&["daemon", "status"])["pid"], owner_pid);
    assert_eq!(peer.json(&["daemon", "status"])["pid"], peer_pid);
}

#[test]
fn grant_covers_all_boards_and_each_daemon_reports_membership() {
    let directory = tempfile::tempdir().unwrap();
    let network = directory.path().file_name().unwrap().to_str().unwrap();
    let owner = Host::owner(network);
    let peer = Host::new(network);
    for board in ["Orchard", "Tool Shed"] {
        owner.json(&["board", "new", board]);
        owner.json(&["add", "--board", board, &format!("Inventory {board}")]);
    }
    sync_hosts(&[&owner]);
    owner.json(&["board", "use", "Orchard"]);
    connect(&owner, &peer, "Garden tablet");
    for host in [&owner, &peer] {
        for board in ["Greenhouse", "Orchard", "Tool Shed"] {
            wait_for_board(host, board);
        }
    }
    sync_hosts(&[&owner, &peer]);
    assert_eq!(peer.json(&["status"])["board"]["name"], "Greenhouse");
    assert_eq!(peer.json(&["ls", "--all-boards"])["total"], 2);
    assert_eq!(
        owner.json(&["status"])["boards"].as_array().unwrap().len(),
        3
    );
    assert!(
        peer.json(&["device", "ls"])["devices"]
            .as_array()
            .unwrap()
            .iter()
            .all(|device| { device["boards"].as_array().unwrap().len() == 3 })
    );
}
