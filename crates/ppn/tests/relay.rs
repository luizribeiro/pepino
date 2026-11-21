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

        assert_private_files(&relay.data, &[first_title, second_title, extra_title]);
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
        assert_private_files(
            &relay.data,
            &[first_title, extra_title, second_title, &board],
        );
        a.stop();
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
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let options = SqliteConnectOptions::new().filename(path).read_only(true);
                let mut db = SqliteConnection::connect_with(&options).await.unwrap();
                query(&mut db).await
            })
    }

    fn assert_private_files(dir: &Path, secrets: &[&str]) {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_private_files(&path, secrets);
            } else if path.is_file() {
                let bytes = fs::read(&path).unwrap();
                for secret in secrets {
                    assert!(
                        !bytes
                            .windows(secret.len())
                            .any(|window| window == secret.as_bytes()),
                        "secret found in {}",
                        path.display()
                    );
                }
                assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
            }
        }
    }
}
