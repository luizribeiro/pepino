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
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("use a separate data directory for a device"));
    }
    let device = ConfigSandbox::new();
    success(device.command().args(["init"]).output().unwrap());
    let error = task_error(
        device
            .command()
            .args(["relay", "init", "--json"])
            .output()
            .unwrap(),
        5,
        "permission_denied",
    );
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("is a device; use a separate data directory for a relay"));
    let error = task_error(
        device
            .command()
            .args(["relay", "show", "--json"])
            .output()
            .unwrap(),
        5,
        "permission_denied",
    );
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("is not a relay. Run ppn relay init in an unused data directory."));
}

#[test]
fn relay_board_commands_need_a_relay_directory_and_a_running_daemon() {
    let device = ConfigSandbox::new();
    success(device.command().args(["init"]).output().unwrap());
    let relay = ConfigSandbox::new();
    success(relay.command().args(["relay", "init"]).output().unwrap());
    let run =
        |sandbox: &ConfigSandbox, args: &[&str]| sandbox.command().args(args).output().unwrap();
    let node = "ab".repeat(32);
    let from = ["relay", "drop", "--from", &node, "--yes", "--json"];
    for arguments in [
        &["relay", "boards", "--json"][..],
        &["relay", "drop", "3f9a12c4d5e6", "--yes", "--json"],
        &from,
    ] {
        task_error(run(&device, arguments), 5, "permission_denied");
        let offline = task_error(run(&relay, arguments), 6, "daemon_unreachable");
        assert!(offline["message"].as_str().unwrap().contains("not running"));
    }
    let human = run(&relay, &["relay", "boards"]);
    assert_eq!(human.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&human.stderr).contains("ppn daemon start"));
    task_error(
        run(&relay, &["relay", "drop", "3f9a12c4d5e6", "--json"]),
        1,
        "confirmation_required",
    );
    task_error(
        run(
            &relay,
            &["relay", "drop", "--from", "laptop", "--yes", "--json"],
        ),
        2,
        "invalid_input",
    );
    for usage in [
        &["relay", "drop"][..],
        &["relay", "drop", "3f9a12c4d5e6", "--from", &node],
    ] {
        assert_eq!(run(&relay, usage).status.code(), Some(2));
    }
}

#[cfg(pepino_network_tests)]
#[path = "common/wait.rs"]
mod wait;

#[cfg(pepino_network_tests)]
#[path = "device/support.rs"]
mod support;

#[cfg(pepino_network_tests)]
mod network {
    use super::{
        common::assert_json_error,
        success,
        support::Host,
        wait::{WAIT, until},
    };
    use serde_json::Value;
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
    use std::{fs, path::Path};

    #[test]
    fn two_devices_converge_through_a_restarted_ciphertext_only_relay() {
        let root = tempfile::tempdir().unwrap();
        let name = root.path().file_name().unwrap().to_str().unwrap();
        let b = Host::new(name);
        let request = b.request("Tablet");
        let a = Host::owner(name);
        let relay = Host::new(name);
        let rejected = Host::new(name);
        rejected.run(&[
            "init", "--name", "Bea", "--device", "Desk", "--board", "Workshop",
        ]);
        let a_id = a.json(&["status"])["device"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let address = relay.json(&["relay", "init"])["address"]
            .as_str()
            .unwrap()
            .to_owned();
        let start_relay = || {
            success(
                relay
                    .command()
                    .env("PPN_TRUSTED_DEVICES", &a_id)
                    .env("PPN_MAX_BOARDS", "1")
                    .args(["daemon", "start", "--json"])
                    .output()
                    .unwrap(),
            )
        };

        start_relay();
        for device_file in ["local.json", "pepino.sqlite", "node.sqlite"] {
            assert!(!relay.data.join(device_file).exists());
        }
        let board = a.json(&["board", "show"])["board"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            a.run(&["board", "relay", "ls"])
                .contains("No relay requests")
        );
        let add = ["board", "relay", "add", &address, "--all-boards"];
        assert_eq!(a.json(&add)["boards"][0]["relays"][0]["state"], "requested");
        assert!(a.run(&add).contains(&address));
        wait_state(&a, "Greenhouse", "carrying");
        assert!(a.run(&["board", "show"]).contains("carrying"));
        assert_eq!(
            a.json(&["board", "show"])["board"]["relays"][0]["state"],
            "carrying"
        );

        let grant = a.add(&request, &[]);
        let first_title = "Replace the east roof glazing";
        let extra_title = "Check the east frame seals";
        let created = a.json(&["add", first_title]);
        let task = created["created"].as_str().unwrap();
        a.run(&["add", extra_title]);
        let first_hash = published(&a, first_title);
        let extra_hash = published(&a, extra_title);
        wait_for_ciphertext(&relay, &first_hash);
        wait_for_ciphertext(&relay, &extra_hash);

        rejected.start();
        rejected.run(&["board", "relay", "add", &address, "--board", "Workshop"]);
        let refusal = wait_state(&rejected, "Workshop", "refused");
        assert!(refusal["reason"].as_str().unwrap().contains("not trusted"));
        rejected.stop();
        let status = relay.json(&["status"]);
        assert_eq!(status["relay"]["boards_by_device"][&a_id], 1);
        assert!(status["relay"]["rejected_instructions"].as_u64().unwrap() > 0);
        assert!(status["relay"]["bytes_stored"].as_u64().unwrap() > 0);
        assert!(
            status["relay"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == &a_id)
        );
        assert!(!status.to_string().contains(&board));
        let human = relay.run(&["status"]);
        assert!(human.contains("Carrying 1 of 1 boards"));
        assert!(!human.contains(&board));

        a.stop();
        relay.stop();
        start_relay();
        assert_eq!(
            relay.json(&["status"])["relay"]["boards_by_device"][&a_id],
            1
        );
        assert_eq!(
            b.json(&["device", "join", &grant, "--timeout", "60s"])["joined"],
            true
        );
        wait_task(&b, first_title);
        wait_task(&b, extra_title);
        wait_state(&b, "Greenhouse", "carrying");
        let second_title = "Record the glazing measurements";
        b.run(&[
            "edit",
            task,
            "--title",
            second_title,
            "--board",
            "Greenhouse",
        ]);
        let second_hash = published(&b, second_title);
        wait_for_ciphertext(&relay, &second_hash);
        b.stop();
        a.start();
        wait_task(&a, second_title);

        a.run(&["board", "new", "Seed House"]);
        a.run(&["board", "relay", "add", &address, "--board", "Seed House"]);
        let refusal = wait_state(&a, "Seed House", "refused");
        assert!(refusal["reason"].as_str().unwrap().contains("board limit"));

        let titles = [
            first_title.as_bytes(),
            second_title.as_bytes(),
            extra_title.as_bytes(),
        ];
        assert_private_files(&relay.data, &titles);
        let rm = ["board", "relay", "rm", &address, "--board", "Greenhouse"];
        let denied = [&rm[..], &["--json"]].concat();
        assert_json_error(a.output(&denied), 1, "confirmation_required");
        let confirmed = [&rm[..], &["--yes"]].concat();
        assert_eq!(a.json(&confirmed)["boards"][0]["relays"][0]["carry"], false);
        assert!(a.run(&confirmed).contains("removal"));
        wait_state(&a, "Greenhouse", "removed");
        assert_eq!(
            relay.json(&["status"])["relay"]["boards_by_device"][&a_id],
            0
        );
        assert_eq!(relay.json(&["status"])["relay"]["bytes_stored"], 0);
        for hash in [&first_hash, &extra_hash, &second_hash] {
            assert!(!has_ciphertext(&relay.data, hash));
        }
        let raw_id: pepino_core::BoardId = board.parse().unwrap();
        assert_private_files(
            &relay.data,
            &[
                titles[0],
                titles[1],
                titles[2],
                board.as_bytes(),
                raw_id.as_bytes(),
            ],
        );
        a.stop();
        relay.stop();
    }

    struct Fleet {
        device: Host,
        device_id: String,
        relay: Host,
        address: String,
    }

    fn fleet() -> Fleet {
        let root = tempfile::tempdir().unwrap();
        let name = root.path().file_name().unwrap().to_str().unwrap();
        let device = Host::owner(name);
        let relay = Host::new(name);
        let device_id = device.json(&["status"])["device"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let address = relay.json(&["relay", "init"])["address"]
            .as_str()
            .unwrap()
            .to_owned();
        success(
            relay
                .command()
                .env("PPN_TRUSTED_DEVICES", &device_id)
                .args(["daemon", "start", "--json"])
                .output()
                .unwrap(),
        );
        Fleet {
            device,
            device_id,
            relay,
            address,
        }
    }

    #[test]
    fn the_relay_daemon_survives_a_failed_instruction() {
        let Fleet {
            device,
            relay,
            address,
            ..
        } = fleet();
        let pid = relay.json(&["daemon", "status"])["pid"].clone();
        with_db(&relay.data.join("relay.sqlite"), false, async |db| {
            sqlx::query("DROP TABLE pepino_relay_ledger")
                .execute(db)
                .await
                .unwrap();
        });
        device.run(&["board", "relay", "add", &address]);
        wait_state(&device, "Greenhouse", "carrying");
        assert_eq!(relay.json(&["daemon", "status"])["pid"], pid);
        let log = fs::read_to_string(relay.data.join("daemon.log")).unwrap();
        assert!(
            log.contains("the relay node failed: node storage:"),
            "{log}"
        );
        assert!(log.contains("restarting the relay node"), "{log}");
        device.stop();
        relay.stop();
    }

    fn carried(relay: &Host) -> Option<u64> {
        let output = relay.output(&["relay", "boards", "--json"]);
        serde_json::from_slice::<Value>(&output.stdout).ok()?["count"].as_u64()
    }

    #[test]
    fn the_operator_lists_and_drops_carried_boards() {
        let Fleet {
            device,
            device_id,
            relay,
            address,
        } = fleet();
        let pid = relay.json(&["daemon", "status"])["pid"].clone();
        let board = device.json(&["board", "show"])["board"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        device.run(&["board", "new", "Seed House"]);
        device.run(&["board", "relay", "add", &address, "--all-boards"]);
        wait_state(&device, "Greenhouse", "carrying");
        wait_state(&device, "Seed House", "carrying");

        let listing = relay.json(&["relay", "boards"]);
        assert_eq!(listing["kind"], "relay_boards");
        assert_eq!(listing["count"], 2);
        assert_eq!(listing["max_boards"], 64);
        assert_eq!(listing["devices"][0]["node_id"], device_id.as_str());
        let handles: Vec<_> = listing["devices"][0]["boards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|board| board["handle"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(handles.len(), 2);
        let human = relay.run(&["relay", "boards"]);
        assert!(human.contains("Carrying 2 of 64 boards"));
        assert!(human.contains(&format!("{device_id}  2 boards")));
        for text in [&human, &listing.to_string()] {
            assert!(handles.iter().all(|handle| text.contains(handle)));
            assert!(!text.contains(&board));
        }

        let drop = ["relay", "drop", &handles[0]];
        assert_json_error(
            relay.output(&[&drop[..], &["--json"]].concat()),
            1,
            "confirmation_required",
        );
        assert_json_error(
            relay.output(&["relay", "drop", "0123456789ab", "--yes", "--json"]),
            3,
            "not_found",
        );
        assert_eq!(carried(&relay), Some(2));
        let dropped = relay.json(&[&drop[..], &["--yes"]].concat());
        assert_eq!(dropped["kind"], "relay_drop");
        assert_eq!(dropped["dropped"], 1);
        until(|| carried(&relay), |count| *count == Some(1), WAIT);
        let remaining = until(
            || relay.output(&["relay", "drop", "--from", &device_id, "--yes"]),
            |output| output.status.success(),
            WAIT,
        );
        assert!(String::from_utf8_lossy(&remaining.stdout).contains("Dropped 1 boards"));
        until(|| carried(&relay), |count| *count == Some(0), WAIT);
        assert_eq!(relay.json(&["daemon", "status"])["pid"], pid);
        let raw_id: pepino_core::BoardId = board.parse().unwrap();
        assert_private_files(&relay.data, &[board.as_bytes(), raw_id.as_bytes()]);

        device.run(&["board", "relay", "add", &address, "--board", "Greenhouse"]);
        wait_state(&device, "Greenhouse", "carrying");
        until(|| carried(&relay), |count| *count == Some(1), WAIT);
        device.stop();
        relay.stop();
    }

    fn wait_state(host: &Host, board: &str, state: &str) -> Value {
        until(
            || {
                host.json(&["board", "relay", "ls", "--board", board])["boards"][0]["relays"][0]
                    .clone()
            },
            |entry| entry["state"] == state,
            WAIT,
        )
    }

    fn wait_task(host: &Host, title: &str) {
        until(
            || host.run(&["ls", "--json", "--board", "Greenhouse"]),
            |listing| listing.contains(title),
            WAIT,
        );
    }

    fn published(host: &Host, title: &str) -> String {
        host.run(&["sync", "--timeout", "40s"]);
        read_db(&host.data.join("pepino.sqlite"), async |db| {
            sqlx::query_scalar::<_, String>(
                "SELECT lower(hex(operation)) FROM publication
                 JOIN event_log AS e ON e.board = publication.board AND e.id = publication.event
                 WHERE instr(e.bytes, ?) > 0",
            )
            .bind(title.as_bytes())
            .fetch_one(db)
            .await
            .unwrap()
        })
    }

    fn wait_for_ciphertext(host: &Host, hash: &str) {
        until(|| has_ciphertext(&host.data, hash), |found| *found, WAIT);
    }

    fn has_ciphertext(dir: &Path, hash: &str) -> bool {
        read_db(&dir.join("relay.sqlite"), async |db| {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM operations_v1 WHERE hash = ? AND length(body)>0)",
            )
            .bind(hash)
            .fetch_one(db)
            .await
            .unwrap()
        })
    }

    fn read_db<T>(path: &Path, query: impl AsyncFnOnce(&mut SqliteConnection) -> T) -> T {
        with_db(path, true, query)
    }

    fn with_db<T>(
        path: &Path,
        read_only: bool,
        query: impl AsyncFnOnce(&mut SqliteConnection) -> T,
    ) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let options = SqliteConnectOptions::new()
                    .filename(path)
                    .read_only(read_only);
                let mut db = SqliteConnection::connect_with(&options).await.unwrap();
                query(&mut db).await
            })
    }

    fn assert_private_files(dir: &Path, secrets: &[&[u8]]) {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_private_files(&path, secrets);
            } else if path.is_file() {
                let bytes = fs::read(&path).unwrap();
                for secret in secrets {
                    assert!(
                        !bytes.windows(secret.len()).any(|window| window == *secret),
                        "secret found in {}",
                        path.display()
                    );
                }
                assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
            }
        }
    }
}
