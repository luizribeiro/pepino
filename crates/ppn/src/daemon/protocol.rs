use std::collections::BTreeMap;

use pepino_app::{BoardGrant, BoardJoinState, BoardRequest, BoardRole, DeviceRequest, PeerHint};
use pepino_core::{BoardId, DeviceId};
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
    DeviceRemoval {
        device: DeviceId,
        timeout_ms: u64,
    },
    RegisterPeerHint {
        peer: PeerHint,
    },
    BoardJoinState {
        timeout_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        board: Option<BoardId>,
    },
    BoardRequest {
        name: Option<String>,
    },
    BoardInvite {
        board: BoardId,
        request: Box<BoardRequest>,
        role: BoardRole,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        relay_url: Option<String>,
    },
    BoardManage {
        board: BoardId,
        person: pepino_app::PersonKey,
        action: pepino_app::ManagePerson,
        accept_risk: bool,
    },
    RelayDrop {
        boards: pepino_node::RelayDrop,
    },
    Stop,
}

impl Operation {
    pub(super) fn wait_ms(&self) -> Option<u64> {
        match *self {
            Self::WaitIdle { timeout_ms }
            | Self::BoardJoinState { timeout_ms, .. }
            | Self::DeviceRemoval { timeout_ms, .. } => Some(timeout_ms),
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
        #[serde(default)]
        causes: Vec<String>,
    },
    MemberAccessDenied {
        board: String,
    },
    DeviceRemoval {
        boards: Option<Vec<pepino_sync::BoardRemoval>>,
    },
    PeerHintRegistered,
    BoardJoinState {
        boards: BTreeMap<BoardId, BoardJoinState>,
    },
    BoardRequest {
        request: Box<BoardRequest>,
    },
    BoardInvited {
        grant: Box<BoardGrant>,
    },
    BoardManaged {
        departure: Option<pepino_app::BoardDeparture>,
    },
    RelayDropped {
        dropped: pepino_node::DroppedBoards,
    },
    Stopping,
    Error {
        code: String,
        message: String,
    },
}

impl Outcome {
    pub(super) fn ends_serving(&self) -> Option<super::server::End> {
        match self {
            Self::Stopping => Some(super::server::End::Stopped),
            Self::RelayDropped { dropped } if dropped.purged > 0 => {
                Some(super::server::End::Restart)
            }
            _ => None,
        }
    }

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<pepino_node::RelayStatus>,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub board_members: BTreeMap<BoardId, std::collections::BTreeSet<pepino_core::DeviceId>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Board {
    #[serde(default)]
    pub starting_up: bool,
    #[serde(default)]
    pub enrollment_pending: bool,
    #[serde(default)]
    pub active: bool,
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
        Operation::BoardJoinState {
            timeout_ms: 250,
            board: None
        }
    ));
}

#[cfg(test)]
#[test]
fn a_device_removal_wait_round_trips_the_full_device_id() {
    use serde_json::json;
    let device = DeviceId::from_bytes([67; 32]);
    let wire =
        json!({"v": 1, "request": {"op": "device_removal", "device": device, "timeout_ms": 6000}});
    let request: Request = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(request.request.wait_ms(), Some(6000));
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[cfg(test)]
#[test]
fn a_board_request_control_message_has_no_capabilities() {
    let request = Request {
        v: VERSION,
        request: Operation::BoardRequest {
            name: Some("Garden tablet".into()),
        },
    };
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        serde_json::json!({
            "v": 1, "request": { "op": "board_request", "name": "Garden tablet" }
        })
    );
}

#[cfg(test)]
#[test]
fn a_relay_drop_restarts_the_node_only_when_a_copy_must_be_purged() {
    let dropped = |purged| Outcome::RelayDropped {
        dropped: pepino_node::DroppedBoards { boards: 2, purged },
    };
    assert!(dropped(0).ends_serving().is_none());
    assert!(matches!(
        dropped(1).ends_serving(),
        Some(super::server::End::Restart)
    ));
}
