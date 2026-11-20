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
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

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
        let stopped = as_json(&fixture.run(&["daemon", "stop", "--json"]));
        assert_eq!(stopped["running"], false);
        let exit = as_json(&success(fixture.wait_foreground()));
        assert_eq!(exit["action"], "stopped");
        fixture.assert_stopped();
    }
}
