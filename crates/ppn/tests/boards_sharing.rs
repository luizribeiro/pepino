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
    let house_tasks = luiz.json(&["ls", "board:House"]);
    let guest_task = house_tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == second)
        .unwrap();
    assert_eq!(
        house_tasks["names"][guest_task["author"].as_str().unwrap()],
        "Ana",
        "{house_tasks:#}"
    );
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

#[test]
fn a_read_invitation_is_pending_offline_and_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Ada", "Laptop", "Private work");
    let reader = host(network, "Ada", "Phone", "Private travel");
    owner.run(&["board", "new", "House"]);
    let task = owner.json(&["add", "--board", "House", "Repair the door"]);
    let task = task["created"].as_str().unwrap();
    owner.run(&["comment", "--board", "House", task, "Fitted the old latch"]);
    let request = reader.json(&["board", "request"]);
    let wrong = with_code(
        &owner,
        &[
            "board",
            "invite",
            "-",
            "--board",
            "House",
            "--fingerprint",
            "wrong",
            "--json",
        ],
        request["request"].as_str().unwrap(),
    );
    let error = common::task_error(wrong, 2, "invalid_input");
    assert!(error["message"].as_str().unwrap().contains("fingerprint"));
    let wrong_kind = with_code(
        &reader,
        &["board", "join", "-", "--json"],
        request["request"].as_str().unwrap(),
    );
    common::task_error(wrong_kind, 2, "invalid_input");
    let original = grant(&owner, "House", "read", &request);
    let grant_code = original["grant"]["code"].as_str().unwrap();
    let different = host(network, "Ben", "Tablet", "Private errands");
    different.json(&["board", "request"]);
    common::task_error(
        with_code(&different, &["board", "join", "-", "--json"], grant_code),
        2,
        "invalid_input",
    );
    owner.stop();
    common::task_error(
        with_code(
            &reader,
            &["board", "join", "-", "--json", "--timeout", "250ms"],
            grant_code,
        ),
        7,
        "pending",
    );
    assert_eq!(board(&reader, "House")["shared"], true);
    let denied = common::task_error(
        reader.output(&[
            "comment",
            "--board",
            "House",
            "does-not-exist",
            "-",
            "--json",
        ]),
        5,
        "permission_denied",
    );
    assert!(denied["message"].as_str().unwrap().contains("read role"));
    assert!(denied["message"].as_str().unwrap().contains("House"));
    owner.start();
    until(
        || board(&reader, "House"),
        |value| value["membership"] == "joined",
        WAIT,
    );
    let expected = owner.json(&["show", "--board", "House", task])["names"].clone();
    until(
        || {
            let output = reader.output(&["show", "--board", "House", task, "--json"]);
            output.status.success().then(|| {
                common::as_json(&String::from_utf8(output.stdout).unwrap())["names"].clone()
            })
        },
        |names| names.as_ref() == Some(&expected),
        WAIT,
    );
    assert!(
        reader
            .run(&["show", "--board", "House", task])
            .contains("Ada#")
    );
    assert!(
        owner
            .run(&["show", "--board", "House", task])
            .contains("Ada#")
    );
    assert_eq!(reader.json(&["ls", "--board", "House"])["total"], 1);
    assert!(
        reader
            .run(&["show", "--board", "House", task])
            .contains("Fitted the old latch")
    );
    let ambiguous = common::task_error(
        owner.output(&["assign", "--board", "House", task, "Ada", "--json"]),
        4,
        "ambiguous_id",
    );
    assert!(
        ambiguous["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate.as_str().unwrap().starts_with("Ada#"))
    );
    common::task_error(
        owner.output(&["ls", "--board", "House", "assignee:Ada", "--json"]),
        4,
        "ambiguous_id",
    );
    common::task_error(
        owner.output(&[
            "board", "role", "Ada", "write", "--board", "House", "--json",
        ]),
        4,
        "ambiguous_id",
    );
    let candidate = ambiguous["candidates"][0].as_str().unwrap();
    owner.run(&["assign", "--board", "House", task, candidate]);
    assert_eq!(
        owner.json(&["ls", "--board", "House", &format!("assignee:{candidate}")])["total"],
        1
    );
    reader.run(&["identity", "rename", "Ada Reader"]);
    until(
        || owner.json(&["board", "members", "House"]),
        |result| {
            result["people"]
                .as_array()
                .unwrap()
                .iter()
                .any(|person| person["name"] == "Ada Reader")
        },
        WAIT,
    );
}

#[test]
fn a_read_invite_keeps_the_manager_daemon_available() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Ada", "Laptop", "Private work");
    let reader = host(network, "Ben", "Phone", "Private travel");
    let writer = host(network, "Cora", "Tablet", "Private errands");
    let hosts = [&owner, &reader, &writer];
    let pids = hosts.map(|host| host.json(&["daemon", "status"])["pid"].as_u64().unwrap());
    owner.run(&["board", "new", "House"]);
    for (guest, role) in [(&reader, "read"), (&writer, "write")] {
        let issued = grant(&owner, "House", role, &guest.json(&["board", "request"]));
        guest.run(&[
            "board",
            "join",
            issued["grant"]["code"].as_str().unwrap(),
            "--timeout",
            "60s",
        ]);
    }
    reader.run(&["identity", "rename", "Ben Reader"]);
    until(
        || owner.run(&["board", "members", "House"]),
        |people| people.contains("Ben Reader (read)"),
        WAIT,
    );
    assert_eq!(
        hosts.map(|host| host.json(&["daemon", "status"])["pid"].as_u64().unwrap()),
        pids
    );
}

#[test]
fn a_request_cannot_enroll_its_device_on_a_second_board() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Ada", "Laptop", "Private work");
    let guest = host(network, "Ben", "Phone", "Private travel");
    owner.run(&["board", "new", "House"]);
    owner.run(&["board", "new", "Garden"]);
    let request = guest.json(&["board", "request"]);
    let issued = grant(&owner, "House", "write", &request);
    let error = common::task_error(
        with_code(
            &owner,
            &[
                "board", "invite", "-", "--board", "Garden", "--yes", "--json",
            ],
            request["request"].as_str().unwrap(),
        ),
        2,
        "invalid_input",
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("already been used")
    );
    assert!(!owner.run(&["board", "members", "Garden"]).contains("Ben"));
    let code = issued["grant"]["code"].as_str().unwrap();
    guest.run(&["board", "join", code, "--timeout", "60s"]);
    assert!(
        guest
            .run(&["board", "join", code])
            .contains("Initial sync complete")
    );
    let state = std::fs::read_to_string(guest.data.join("local.json")).unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert!(state["catalog"].get("board_request").is_none());
}

#[test]
fn the_inviter_rejects_an_expired_member_bundle() {
    use data_encoding::BASE32_NOPAD_NOCASE;
    use pepino_app::BoardRequest;
    use ppn::device_code::Kind;

    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Luiz", "Desktop", "Owner private");
    let guest = host(network, "Ana", "Tablet", "Guest private");
    owner.run(&["board", "new", "House"]);
    let request = guest.json(&["board", "request"]);
    let mut expired: BoardRequest = Kind::BoardRequest
        .decode(request["request"].as_str().unwrap())
        .unwrap();
    let mut bundle = expired.device.member_info().unwrap().as_bytes().to_vec();
    // p2panda packs the lifetime between the three public keys and the signatures.
    bundle[97..105].copy_from_slice(&1_u64.to_be_bytes());
    bundle[105..113].copy_from_slice(&2_u64.to_be_bytes());
    expired.device.member_info = BASE32_NOPAD_NOCASE.encode(&bundle);
    let expired = Kind::BoardRequest.encode(&expired).unwrap();
    let rejected = with_code(
        &owner,
        &[
            "board", "invite", "-", "--board", "House", "--yes", "--json",
        ],
        &expired,
    );
    let error = common::task_error(rejected, 2, "invalid_input");
    assert!(error["message"].as_str().unwrap().contains("expired"));
}

#[test]
fn a_linked_device_can_revoke_the_device_that_accepted_a_share() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Ada", "Laptop", "Owner private");
    let guest = host(network, "Ben", "Phone", "Guest private");
    let linked = Host::new(network);
    owner.run(&["board", "new", "House"]);
    let code = grant(&owner, "House", "write", &guest.json(&["board", "request"]));
    guest.run(&[
        "board",
        "join",
        code["grant"]["code"].as_str().unwrap(),
        "--timeout",
        "60s",
    ]);
    let request = linked.request("Linked tablet");
    linked.run(&[
        "device",
        "join",
        &guest.add(&request, &[]),
        "--timeout",
        "60s",
    ]);
    until(
        || board(&linked, "House"),
        |row| row["membership"] == "joined",
        WAIT,
    );
    let report = linked.json(&["device", "revoke", "Phone", "--yes", "--timeout", "60s"]);
    let rows = report["boards"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["board"]["name"] == "Guest private" && row["result"] == "removed")
    );
    assert!(rows.iter().any(|row| row["board"]["name"] == "House"
        && matches!(row["result"].as_str(), Some("manager_required" | "removed"))));
    until(
        || member(&owner, "Ben"),
        |person| {
            person["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|device| device["name"] == "Phone" && device["membership"] == "removed")
        },
        WAIT,
    );
}

#[test]
fn private_catalogs_and_device_links_remain_local_to_each_identity() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Luiz", "Desktop", "Owner private");
    let guest = host(network, "Ana", "Tablet", "Guest private");
    let guest_private = guest.json(&["board", "new", "Guest notes"]);
    let private_id = guest_private["board"]["id"].as_str().unwrap();
    guest.run(&[
        "add",
        "--board",
        "Guest notes",
        "Private family appointments",
    ]);
    owner.run(&["board", "new", "House"]);
    let request = guest.json(&["board", "request"]);
    let code = grant(&owner, "House", "write", &request);
    assert!(
        guest
            .run(&[
                "board",
                "join",
                code["grant"]["code"].as_str().unwrap(),
                "--timeout",
                "60s"
            ])
            .contains("Initial sync complete")
    );
    for source in [&guest, &owner] {
        let linked = Host::new(network);
        let request = linked.request("Linked tablet");
        let grant = source.add(&request, &[]);
        linked.run(&["device", "join", &grant, "--timeout", "60s"]);
        until(
            || board(&linked, "House"),
            |board| board["membership"] == "joined",
            WAIT,
        );
        let report = source.json(&[
            "device",
            "revoke",
            "Linked tablet",
            "--yes",
            "--timeout",
            "60s",
        ]);
        assert_eq!(report["kind"], "device_revoke");
    }
    let listing = owner.json(&["ls", "--all-boards"]).to_string();
    assert!(!listing.contains("Private family appointments"));
    assert!(
        !owner
            .json(&["board", "ls"])
            .to_string()
            .contains(private_id)
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
        let mut db = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(owner.data.join("node.sqlite"))
                .read_only(true),
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM spaces_v1 WHERE id = ?")
            .bind(private_id)
            .fetch_one(&mut db)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let app = pepino_app::App::open(&owner.data, 0).await.unwrap();
        let private_board = app
            .store()
            .board(private_id.parse().unwrap())
            .await
            .unwrap();
        assert!(private_board.tasks().is_empty());
    });
}

fn join(owner: &Host, guest: &Host, role: &str) {
    let issued = grant(owner, "House", role, &guest.json(&["board", "request"]));
    let output = guest.output(&[
        "board",
        "join",
        issued["grant"]["code"].as_str().unwrap(),
        "--timeout",
        "60s",
    ]);
    assert!(
        output.status.success(),
        "board join failed {output:?}\n{}",
        std::fs::read_to_string(guest.data.join("daemon.log")).unwrap_or_default()
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Initial sync complete"));
}

fn member(owner: &Host, name: &str) -> Value {
    owner.json(&["board", "members", "House"])["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|person| person["name"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn a_reader_links_a_device_and_both_follow_person_roles_and_departure() {
    let dir = tempfile::tempdir().unwrap();
    let network = dir.path().file_name().unwrap().to_str().unwrap();
    let owner = host(network, "Ada", "Laptop", "Owner private");
    let guest = host(network, "Ben", "Phone", "Guest private");
    let linked = Host::new(network);
    owner.run(&["board", "new", "House"]);
    let task = owner.json(&["add", "--board", "House", "Repair the window"]);
    let id = task["created"].as_str().unwrap();
    join(&owner, &guest, "read");
    let request = linked.request("Linked tablet");
    linked.run(&[
        "device",
        "join",
        &guest.add(&request, &[]),
        "--timeout",
        "60s",
    ]);
    until(
        || board(&linked, "House"),
        |board| board["membership"] == "joined",
        WAIT,
    );
    until(
        || member(&owner, "Ben"),
        |person| {
            person["devices"].as_array().is_some_and(|devices| {
                devices.len() == 2
                    && devices
                        .iter()
                        .all(|device| device["membership"] == "member" && device["role"] == "read")
            })
        },
        WAIT,
    );
    common::task_error(
        guest.output(&["board", "role", "me", "write", "--board", "House", "--json"]),
        5,
        "permission_denied",
    );
    assert!(
        owner
            .run(&["board", "role", "Ben", "write", "--board", "House"])
            .contains("write role")
    );
    until(
        || {
            [&guest, &linked].map(|host| {
                host.json(&["status"])["boards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["name"] == "House")
                    .unwrap()["role"]
                    .clone()
            })
        },
        |roles| roles == &["write", "write"],
        WAIT,
    );
    linked.run(&[
        "comment",
        "--board",
        "House",
        id,
        "The replacement glass is ready",
    ]);
    owner.run(&["assign", "--board", "House", id, "Ben"]);
    until(
        || owner.run(&["show", "--board", "House", id]),
        |task| task.contains("The replacement glass is ready"),
        WAIT,
    );
    let changed = owner.json(&["board", "role", "Ben", "read", "--board", "House"]);
    assert_eq!(changed["kind"], "board_role");
    until(
        || {
            [&guest, &linked].map(|host| {
                host.json(&["status"])["boards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["name"] == "House")
                    .unwrap()["role"]
                    .clone()
            })
        },
        |roles| roles == &["read", "read"],
        WAIT,
    );
    common::task_error(
        linked.output(&["add", "--board", "House", "Should fail", "--json"]),
        5,
        "permission_denied",
    );
    let confirmation = common::task_error(
        owner.output(&["board", "remove", "Ben", "--board", "House", "--json"]),
        1,
        "confirmation_required",
    );
    assert!(
        confirmation["message"]
            .as_str()
            .unwrap()
            .contains("full history")
    );
    let removed = owner.json(&["board", "remove", "Ben", "--board", "House", "--yes"]);
    assert_eq!(removed["kind"], "board_remove");
    for host in [&guest, &linked] {
        until(
            || board(host, "House"),
            |board| board["membership"] == "removed",
            WAIT,
        );
        common::task_error(
            host.output(&["add", "--board", "House", "Should fail", "--json"]),
            5,
            "permission_denied",
        );
        let local = host.output(&["show", "--board", "House", id]);
        assert!(local.status.success(), "{local:?}");
        assert!(String::from_utf8_lossy(&local.stdout).contains("Repair the window"));
        assert!(String::from_utf8_lossy(&local.stderr).contains("membership removed"));
    }
    let saved = owner.run(&["show", "--board", "House", id]);
    assert!(saved.contains("former member") && saved.contains("The replacement glass is ready"));
    let history = owner.json(&["log", "--board", "House", "--all"]);
    assert!(
        history["membership_history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| change["action"] == "removed")
    );
    assert!(
        owner
            .run(&["board", "members", "House"])
            .contains("Ben (former member)")
    );
}
