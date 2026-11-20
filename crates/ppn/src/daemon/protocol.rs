use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub request: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Nudge,
    Status,
    WaitIdle { timeout_ms: u64 },
    Stop,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub result: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Nudged,
    Status { status: DaemonStatus },
    Synced { idle: bool, network: Network },
    Stopping,
    Error { code: String, message: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub uptime_ms: u64,
    pub network: Network,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Network {
    pub peers_seen: Vec<String>,
    pub peers_connected: Vec<String>,
    pub boards: BTreeMap<String, Board>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Board {
    pub events_waiting: Option<u64>,
    pub published: u64,
    pub received: u64,
    pub last_sent: Option<chrono::DateTime<chrono::Utc>>,
    pub last_received: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_a_versioned_idle_wait_and_reply() {
        let request = json!({"v": 1, "request": {"op": "wait_idle", "timeout_ms": 1200}});
        let parsed: Request = serde_json::from_value(request.clone()).unwrap();
        assert!(matches!(
            parsed.request,
            Operation::WaitIdle { timeout_ms: 1200 }
        ));
        assert_eq!(serde_json::to_value(parsed).unwrap(), request);
        let reply = Response {
            v: VERSION,
            result: Outcome::Stopping,
        };
        assert_eq!(
            serde_json::to_value(reply).unwrap(),
            json!({"v": 1, "result": {"kind": "stopping"}})
        );
    }
}
