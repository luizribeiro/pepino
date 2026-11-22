use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::protocol::DaemonStatus;
use crate::{output::Render, terminal::Terminal};

pub struct State {
    pub action: &'static str,
    pub already_stopped: bool,
    pub status: Option<DaemonStatus>,
    pub socket_path: PathBuf,
}

impl Render for State {
    fn human(&self, _: &Terminal) -> String {
        match (&self.status, self.action) {
            (Some(status), _) if let Some(elapsed) = status.network.starting_ms => {
                describe_startup(elapsed)
            }
            (Some(status), "started") => format!("Daemon running (PID {})", status.pid),
            (Some(status), _) => format!(
                "Daemon running (PID {}, uptime {})\nSocket {}",
                status.pid,
                humantime::format_duration(std::time::Duration::from_millis(status.uptime_ms)),
                self.socket_path.display()
            ),
            (None, "stopped") if self.already_stopped => "The daemon is not running.".into(),
            (None, "stopped") => "Daemon stopped".into(),
            (None, _) => format!("Daemon not running\nSocket {}", self.socket_path.display()),
        }
    }

    fn json_fields(self) -> Map<String, Value> {
        let mut fields = Map::from_iter([
            ("kind".into(), json!("daemon")),
            ("action".into(), json!(self.action)),
            ("running".into(), json!(self.status.is_some())),
            (
                "pid".into(),
                json!(self.status.as_ref().map(|status| status.pid)),
            ),
            (
                "uptime_ms".into(),
                json!(self.status.as_ref().map(|status| status.uptime_ms)),
            ),
            (
                "socket_path".into(),
                json!(self.socket_path.to_string_lossy()),
            ),
        ]);
        if let Some(elapsed) = self.status.and_then(|status| status.network.starting_ms) {
            fields.insert("starting_ms".into(), json!(elapsed));
        }
        fields
    }
}

pub fn describe_startup(elapsed: u64) -> String {
    let elapsed = humantime::format_duration(std::time::Duration::from_secs(elapsed / 1000));
    format!("Daemon still starting ({elapsed} elapsed)")
}

#[cfg(test)]
#[test]
fn reports_an_inactive_daemon_with_its_socket_path() {
    let state = State {
        action: "status",
        already_stopped: false,
        status: None,
        socket_path: "/srv/release/control.sock".into(),
    };
    let fields = state.json_fields();
    assert_eq!(fields["running"], false);
    assert_eq!(fields["socket_path"], "/srv/release/control.sock");
    assert!(fields["pid"].is_null());
}
