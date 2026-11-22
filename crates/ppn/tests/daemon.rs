mod common;
use common::*;

#[test]
fn daemon_commands_report_missing_initialization_without_creating_state() {
    let (sandbox, data) = uninitialized_task_sandbox();
    for action in ["run", "start", "status"] {
        let output = sandbox
            .command()
            .args(["daemon", action, "--json"])
            .output()
            .unwrap();
        assert_eq!(
            task_error(output, 1, "not_initialized")["hint"],
            "Run `ppn init`."
        );
        assert!(!data.exists());
    }
}

#[test]
fn stopping_an_inactive_daemon_succeeds_in_a_pipe() {
    for (sandbox, _) in [task_sandbox(), uninitialized_task_sandbox()] {
        assert_eq!(
            sandbox.run(&["daemon", "stop"]).trim(),
            "The daemon is not running."
        );
        let stopped = as_json(&sandbox.run(&["daemon", "stop", "--json"]));
        assert_eq!(stopped["action"], "stopped");
        assert_eq!(stopped["running"], false);
        assert!(stopped["pid"].is_null());
    }
}

#[cfg(pepino_network_tests)]
#[path = "daemon_support/mod.rs"]
mod support;

#[cfg(pepino_network_tests)]
#[path = "common/wait.rs"]
mod wait;

#[cfg(pepino_network_tests)]
#[test]
fn daemon_start_reports_a_corrupt_node_database() {
    let fixture = support::Fixture::new("");
    std::fs::write(fixture.data.join("node.sqlite"), b"invalid SQLite header").unwrap();
    for json in [false, true] {
        let mut command = fixture.command();
        command.args(["daemon", "start"]);
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        let message = if json {
            task_error(output, 6, "daemon_unreachable")["message"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            assert_eq!(output.status.code(), Some(6), "{output:?}");
            String::from_utf8(output.stderr).unwrap()
        };
        assert!(
            message.contains("Could not start the network node"),
            "{message}"
        );
        assert!(message.contains("not a database"), "{message}");
    }
}

#[cfg(pepino_network_tests)]
mod network {
    use super::support::Fixture;
    use super::*;
    use nix::sys::signal::Signal;
    use serde_json::json;
    use std::{net::UdpSocket, os::unix::fs::PermissionsExt};

    #[test]
    fn status_and_daemon_report_a_newer_catalog_once() {
        #[derive(serde::Serialize)]
        struct FutureCatalog {
            v: u32,
            identity: pepino_core::IdentityId,
            new_field: bool,
        }

        let mut fixture = Fixture::new("");
        let path = &fixture.sandbox.config;
        let config = std::fs::read_to_string(path)
            .unwrap()
            .replace("log_level = 'off'", "log_level = 'warn'");
        std::fs::write(path, config).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let version = pepino_app::PROTOCOL_VERSION + 1;
        runtime.block_on(async {
            let now = pepino_app::wall_time_ms();
            let app = pepino_app::App::open(&fixture.data, now).await.unwrap();
            let local = app.local_state();
            let mut bytes = b"pepino-board-catalog\0".to_vec();
            bytes.extend(
                pepino_app::WireFormat::Cbor
                    .encode(&FutureCatalog {
                        v: version,
                        identity: local.identity_id,
                        new_field: true,
                    })
                    .unwrap(),
            );
            for _ in 0..2 {
                app.receive_catalog(local.catalog_board(), local.device_id, &bytes, now)
                    .await
                    .unwrap();
            }
        });

        let message = "another of your devices runs a newer pepino; upgrade this one";
        assert!(fixture.run(&["status"]).contains(message));
        let status = as_json(&fixture.run(&["status", "--json"]));
        assert_eq!(
            status["newer_catalog"],
            json!({"version":version,"message":message})
        );

        fixture.foreground();
        fixture.run(&["daemon", "status"]);
        fixture.run(&["daemon", "status"]);
        fixture.run(&["daemon", "stop"]);
        let output = fixture.wait_foreground();
        assert!(output.status.success(), "{output:?}");
        let log = String::from_utf8(output.stderr).unwrap();
        assert_eq!(log.matches(message).count(), 1, "{log}");
    }

    #[test]
    fn serves_control_requests_refuses_a_second_instance_and_stops() {
        let mut fixture = Fixture::new("");
        let pid = fixture.foreground();
        assert_eq!(
            std::fs::metadata(&fixture.socket)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fixture
                .run(&["daemon", "status"])
                .contains(&format!("PID {pid}"))
        );
        let status = as_json(&fixture.run(&["daemon", "status", "--json"]));
        assert_eq!(status["socket_path"], json!(fixture.socket));
        assert!(status["uptime_ms"].is_u64());
        let second = fixture
            .command()
            .args(["daemon", "run", "--json"])
            .output()
            .unwrap();
        assert!(
            assert_json_error(second, 1, "busy")["error"]["message"]
                .as_str()
                .unwrap()
                .contains("already running")
        );
        let loser = fixture
            .command()
            .env("PPN_INTERNAL_DETACH", "1")
            .args(["daemon", "run"])
            .output()
            .unwrap();
        assert!(loser.status.success());
        assert!(loser.stderr.is_empty());
        let reply = fixture.raw(&json!({"v":2,"request":{"op":"next_operation"}}));
        assert_eq!(reply["result"]["code"], "unsupported_version");
        let stopped = as_json(&fixture.run(&["daemon", "stop", "--json"]));
        assert_eq!(stopped["running"], false);
        let exit = as_json(&success(fixture.wait_foreground()));
        assert_eq!(exit["action"], "stopped");
        fixture.assert_stopped();
    }

    #[test]
    fn sigterm_releases_the_fixed_udp_port_and_removes_the_socket() {
        let reserved = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = reserved.local_addr().unwrap().port();
        let mut fixture = Fixture::new(&format!("port = {port}\n"));
        drop(reserved);
        fixture.foreground();
        assert!(UdpSocket::bind(("0.0.0.0", port)).is_err());
        fixture.signal(Signal::SIGTERM);
        success(fixture.wait_foreground());
        fixture.assert_stopped();
        UdpSocket::bind(("0.0.0.0", port)).unwrap();
    }

    #[test]
    fn detached_start_recovers_the_socket_after_sigkill() {
        let mut fixture = Fixture::new("");
        let dead = fixture.foreground();
        fixture.signal(Signal::SIGKILL);
        assert!(!fixture.wait_foreground().status.success());
        assert!(fixture.socket.exists());
        assert_eq!(
            fixture.run(&["daemon", "stop"]).trim(),
            "The daemon is not running."
        );
        let running = as_json(&fixture.run(&["daemon", "start", "--json"]));
        let pid = running["pid"].as_u64().unwrap();
        assert_ne!(pid, u64::from(dead));
        assert!(
            fixture
                .run(&["daemon", "start"])
                .contains(&format!("PID {pid}"))
        );
        assert_eq!(fixture.run(&["daemon", "stop"]).trim(), "Daemon stopped");
        fixture.assert_stopped();
        let stopped = as_json(&fixture.run(&["daemon", "stop", "--json"]));
        assert_eq!(stopped["action"], "stopped");
        assert_eq!(stopped["running"], false);
    }
}
#[test]
fn sync_reports_when_startup_is_disabled() {
    let (sandbox, _) = task_sandbox();
    let output = sandbox.command().args(["sync", "--json"]).output().unwrap();
    assert_json_error(output, 6, "daemon_unreachable");
    let help = as_json(&run_ppn(&["help", "--json"]));
    assert!(
        help["exit_codes"][1]["error_codes"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("sync_timeout"))
    );
}

#[cfg(pepino_network_tests)]
#[test]
fn sync_drains_the_local_outbox_without_peers() {
    let fixture = support::Fixture::new("");
    fixture.run(&["add", "Replace the greenhouse latch"]);
    let output = fixture
        .command()
        .env_remove("PPN_NO_DAEMON")
        .args(["sync"])
        .output()
        .unwrap();
    let human = success(output);
    assert!(human.contains("Published 1 event"), "{human}");
    assert!(
        human.contains("Synced with 0 devices.\n0 events waiting to be sent"),
        "{human}"
    );
    let result = as_json(&fixture.run(&["sync", "--json"]));
    assert_eq!(result["schema"], 1);
    assert_eq!(result["idle"], true);
    assert_eq!(result["network"]["peers_connected"], serde_json::json!([]));
    let boards = result["network"]["boards"].as_object().unwrap();
    let (board_id, board) = boards.iter().next().unwrap();
    assert_eq!(board["events_waiting"], 0);
    assert_eq!(board["published"], 0);
    assert_eq!(board["received"], 0);
    assert!(result["synced_devices"].as_array().unwrap().is_empty());
    assert!(fixture.run(&["sync"]).contains("Published 0 events"));
    let lock = std::fs::File::open(fixture.data.join("local.lock")).unwrap();
    lock.lock().unwrap();
    append_task_with_directory_locked(&fixture.data);
    let nudge = fixture.raw(&serde_json::json!({"v":1,"request":{"op":"nudge"}}));
    assert_eq!(nudge["result"]["kind"], "nudged");
    crate::wait::until(
        || fixture.raw(&serde_json::json!({"v":1,"request":{"op":"status"}})),
        |reply| {
            reply["result"]["status"]["network"]["boards"]
                .get(board_id)
                .is_some_and(|board| board["events_waiting"] == 1)
        },
        crate::wait::WAIT,
    );
    let output = fixture
        .command()
        .args(["sync", "--timeout", "500ms", "--json"])
        .output()
        .unwrap();
    let error = assert_json_error(output, 1, "sync_timeout");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("1 event waiting to be sent")
    );
    lock.unlock().unwrap();
    fixture.run(&["sync", "--json"]);
    fixture.run(&["daemon", "stop"]);
    fixture.assert_stopped();
}
#[cfg(pepino_network_tests)]
fn append_task_with_directory_locked(data: &std::path::Path) {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};

    let local = pepino_app::LocalState::load(data).unwrap();
    let event = Envelope::new(
        local.device_id,
        Hlc {
            wall_ms: 1_795_144_800_000,
            counter: 0,
        },
        Event::TaskCreated {
            title: "Test the latch in wet weather".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = pepino_store::Store::open(data.join("pepino.sqlite"))
                .await
                .unwrap();
            store
                .append(
                    local.board_id(),
                    &event.encode().unwrap(),
                    Some(local.device_id),
                )
                .await
                .unwrap();
        });
}

#[cfg(pepino_network_tests)]
#[test]
fn status_combines_daemon_activity_with_local_counts() {
    let mut fixture = support::Fixture::new("");
    assert!(
        fixture
            .run(&["status"])
            .contains("offline (daemon not running)")
    );
    assert_eq!(
        as_json(&fixture.run(&["status", "--json"]))["network"],
        "offline"
    );
    fixture.run(&["add", "Install a rain barrel"]);
    fixture.foreground();
    fixture.run(&["sync"]);
    let human = fixture.run(&["status"]);
    assert!(
        human.contains("network: 0 peers seen, 0 connected"),
        "{human}"
    );
    assert!(human.contains("0 events waiting to be sent"), "{human}");
    assert!(human.contains("Tasks: 1 open, 0 closed"), "{human}");
    let status = as_json(&fixture.run(&["status", "--json"]));
    let board = status["board"]["id"].as_str().unwrap();
    let sync = &status["network"]["boards"][board];
    assert_eq!(status["counts"]["events"], 1);
    assert_eq!(sync["events_waiting"], 0);
    assert!(chrono::DateTime::parse_from_rfc3339(sync["last_sent"].as_str().unwrap()).is_ok());
    assert!(sync["last_received"].is_null());
    fixture.run(&["daemon", "stop"]);
    success(fixture.wait_foreground());
    fixture.assert_stopped();
}
#[test]
fn writes_succeed_with_an_unreachable_socket() {
    let (sandbox, data) = task_sandbox();
    let absent = sandbox.home.path().join("absent/control.sock");
    let mut command = sandbox.command();
    command
        .env_remove("PPN_NO_DAEMON")
        .env("PPN_DAEMON_SOCKET_PATH", &absent);
    let output = command
        .args(["add", "Order the new greenhouse latch", "--json"])
        .output()
        .unwrap();
    assert!(as_json(&success(output))["created"].is_string());
    assert!(!absent.exists());
    assert!(!data.join("daemon.log").exists());
    let verbose = sandbox
        .command()
        .env_remove("PPN_NO_DAEMON")
        .env("PPN_DAEMON_SOCKET_PATH", &absent)
        .args(["add", "Fit the latch on the door", "--verbose"])
        .output()
        .unwrap();
    assert!(verbose.status.success());
    assert!(
        String::from_utf8(verbose.stderr)
            .unwrap()
            .contains("Could not notify the daemon")
    );
}

#[cfg(pepino_network_tests)]
#[test]
fn a_write_autostarts_the_daemon_and_publishes_pending_tasks() {
    use crate::wait::{WAIT, until};

    let fixture = support::Fixture::new("");
    fixture.run(&["add", "Install a rain barrel"]);
    fixture.assert_stopped();
    let output = fixture
        .command()
        .env_remove("PPN_NO_DAEMON")
        .args(["add", "Fix the greenhouse latch"])
        .output()
        .unwrap();
    success(output);
    fixture.ready();
    until(
        || fixture.raw(&serde_json::json!({"v":1,"request":{"op":"status"}})),
        |reply| {
            reply["result"]["status"]["network"]["boards"]
                .as_object()
                .and_then(|boards| boards.values().next())
                .is_some_and(|board| board["published"] == 2)
        },
        WAIT,
    );
    fixture.run(&["daemon", "stop"]);
    fixture.assert_stopped();
}

#[cfg(pepino_network_tests)]
#[test]
fn a_write_sends_a_nudge_to_the_daemon_socket() {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        process::Stdio,
    };

    use crate::wait::{WAIT, until};

    let fixture = support::Fixture::new("");
    let listener = UnixListener::bind(&fixture.socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let child = fixture
        .command()
        .env_remove("PPN_NO_DAEMON")
        .args(["add", "Oil the greenhouse hinges"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut socket, _) = until(|| listener.accept(), Result::is_ok, WAIT).unwrap();
    socket.set_nonblocking(false).unwrap();
    socket.set_read_timeout(Some(WAIT)).unwrap();
    let mut received = String::new();
    BufReader::new(&socket).read_line(&mut received).unwrap();
    assert_eq!(
        as_json(&received),
        serde_json::json!({"v":1,"request":{"op":"nudge"}})
    );
    let _ = writeln!(
        socket,
        "{}",
        serde_json::json!({"v":1,"result":{"kind":"nudged"}})
    );
    success(child.wait_with_output().unwrap());
}

#[cfg(pepino_network_tests)]
#[test]
fn disabling_autostart_in_config_also_prevents_write_notifications() {
    let fixture = support::Fixture::new("[daemon]\nautostart = false\n");
    let output = fixture
        .command()
        .env_remove("PPN_NO_DAEMON")
        .args(["add", "Seed the vegetable bed"])
        .output()
        .unwrap();
    success(output);
    fixture.assert_stopped();
    assert!(!fixture.data.join("daemon.log").exists());
}

#[cfg(all(pepino_network_tests, target_os = "linux"))]
#[test]
fn detached_daemon_leaves_its_working_directory() {
    let fixture = support::Fixture::new("");
    let status = as_json(&fixture.run(&["daemon", "start", "--json"]));
    let pid = status["pid"].as_u64().unwrap();
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/cwd")).unwrap(),
        std::path::Path::new("/")
    );
    fixture.run(&["daemon", "stop"]);
}
