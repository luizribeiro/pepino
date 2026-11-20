mod common;
use common::*;

#[test]
fn daemon_commands_report_missing_initialization_without_creating_state() {
    let (sandbox, data) = uninitialized_task_sandbox();
    for action in ["run", "start", "stop", "status"] {
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

#[cfg(pepino_network_tests)]
#[path = "daemon_support/mod.rs"]
mod support;

#[cfg(pepino_network_tests)]
mod network {
    use super::support::Fixture;
    use super::*;
    use nix::sys::signal::Signal;
    use serde_json::json;
    use std::{net::UdpSocket, os::unix::fs::PermissionsExt};

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
        let output = fixture
            .command()
            .args(["daemon", "stop", "--json"])
            .output()
            .unwrap();
        assert_json_error(output, 6, "daemon_unreachable");
    }
}
