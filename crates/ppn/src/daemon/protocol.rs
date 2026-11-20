use std::collections::BTreeMap;

use pepino_app::{BoardJoinState, DeviceRequest, PeerHint};
use pepino_core::BoardId;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const MAX_WAIT_MS: u64 = 300_000;

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub request: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Nudge,
    CreateBoard {
        name: String,
    },
    Status,
    WaitIdle {
        timeout_ms: u64,
    },
    AddMember {
        device: DeviceRequest,
        name: Option<String>,
    },
    RegisterPeerHint {
        peer: PeerHint,
    },
    BoardJoinState {
        timeout_ms: u64,
    },
    Stop,
}

impl Operation {
    pub(super) fn wait_ms(&self) -> Option<u64> {
        match *self {
            Self::WaitIdle { timeout_ms } | Self::BoardJoinState { timeout_ms } => Some(timeout_ms),
            _ => None,
        }
    }
}

pub fn wait_millis(wait: std::time::Duration) -> Result<u64, crate::error::CliError> {
    use crate::error::{CliError, ErrorKind};
    if wait > std::time::Duration::from_millis(MAX_WAIT_MS) {
        return Err(CliError::new(
            ErrorKind::Usage,
            "The maximum sync or join timeout is 5 minutes.",
        ));
    }
    Ok(wait.as_millis().try_into().unwrap_or(MAX_WAIT_MS))
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
    BoardCreated {
        board: BoardId,
    },
    BoardCreationFailed,
    Status {
        status: DaemonStatus,
    },
    Synced {
        idle: bool,
        network: Network,
    },
    MemberAdded,
    MemberAddIncomplete {
        enrolled: Vec<String>,
        failed: Vec<String>,
    },
    MemberAccessDenied {
        board: String,
    },
    PeerHintRegistered,
    BoardJoinState {
        boards: BTreeMap<BoardId, BoardJoinState>,
    },
    Stopping,
    Error {
        code: String,
        message: String,
    },
}

impl Outcome {
    pub(super) fn error(code: &str, message: &str) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
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
    #[serde(default)]
    pub peers_synced: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub last_seen: BTreeMap<String, chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub board_joins: BTreeMap<BoardId, BoardJoinState>,
    pub boards: BTreeMap<String, Board>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Board {
    #[serde(default)]
    pub starting_up: bool,
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

#[cfg(test)]
#[test]
fn identifies_the_peer_hint_and_join_state_operations() {
    use serde_json::json;

    let id = pepino_core::DeviceId::from_bytes([28; 32]);
    let hint = json!({"v": 1, "request": {"op": "register_peer_hint", "peer": {"id": id, "relay_url": null}}});
    let request: Request = serde_json::from_value(hint.clone()).unwrap();
    assert!(matches!(&request.request, Operation::RegisterPeerHint { peer } if peer.id == id));
    assert_eq!(serde_json::to_value(request).unwrap(), hint);
    let wait = json!({"v": 1, "request": {"op": "board_join_state", "timeout_ms": 250}});
    let request: Request = serde_json::from_value(wait).unwrap();
    assert!(matches!(
        request.request,
        Operation::BoardJoinState { timeout_ms: 250 }
    ));
}
