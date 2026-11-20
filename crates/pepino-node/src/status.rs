use std::collections::HashMap;

use futures_util::{StreamExt, stream::BoxStream};
use p2panda::{
    NodeId,
    streams::{StreamEvent, SystemEvent},
};
use p2panda_net::discovery::DiscoveryEvent;
use pepino_core::{BoardId, DeviceId};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::{Error, NodeStatus, receipt::Payload, status_state::Status};

pub type NodeEvents = BoxStream<'static, Result<NodeEvent, Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncCounts {
    pub incoming_operations: u32,
    pub outgoing_operations: u32,
    pub incoming_bytes: u32,
    pub outgoing_bytes: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NodeEvent {
    Snapshot(NodeStatus),
    PeerConnected {
        peer: DeviceId,
    },
    PeerDisconnected {
        peer: DeviceId,
    },
    BoardOpened {
        board: BoardId,
        replaying: bool,
    },
    BoardClosed {
        board: BoardId,
    },
    ReplayStarted {
        board: BoardId,
        operations: u32,
    },
    ReplayEnded {
        board: BoardId,
        error: Option<String>,
    },
    CaughtUp {
        board: BoardId,
        peer: DeviceId,
    },
    PendingDeliveries {
        board: BoardId,
        count: usize,
    },
    PeerSeen {
        peer: DeviceId,
    },
    SyncStarted {
        board: BoardId,
        peer: DeviceId,
        session_id: u64,
        estimated: SyncCounts,
        topic_sessions: u32,
    },
    SyncEnded {
        board: BoardId,
        peer: DeviceId,
        session_id: u64,
        transferred: SyncCounts,
        error: Option<String>,
    },
}

pub(crate) fn subscribe(events: &broadcast::Sender<NodeEvent>) -> NodeEvents {
    BroadcastStream::new(events.subscribe())
        .map(|event| {
            event.map_err(|error| match error {
                tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count) => {
                    Error::StatusLagged(count)
                }
            })
        })
        .boxed()
}

pub(crate) fn peer_session(
    status: &Status,
    active: &mut HashMap<DeviceId, usize>,
    event: SystemEvent,
) {
    let SystemEvent::Discovery(event) = event else {
        return;
    };
    match event {
        DiscoveryEvent::SessionStarted { remote_node_id, .. } => {
            let peer = device(remote_node_id);
            let count = active.entry(peer).or_default();
            if *count == 0 {
                status.send(NodeEvent::PeerConnected { peer });
            }
            *count += 1;
            status.send(NodeEvent::PeerSeen { peer });
        }
        DiscoveryEvent::SessionEnded { remote_node_id, .. }
        | DiscoveryEvent::SessionFailed { remote_node_id, .. } => {
            let peer = device(remote_node_id);
            if let Some(count) = active.get_mut(&peer) {
                *count -= 1;
                if *count == 0 {
                    active.remove(&peer);
                    status.send(NodeEvent::PeerDisconnected { peer });
                }
            }
        }
    }
}

pub(crate) fn board_status(events: &Status, board: BoardId, event: &StreamEvent<Payload>) {
    let status = match *event {
        StreamEvent::SyncStarted {
            remote_node_id,
            session_id,
            incoming_operations,
            outgoing_operations,
            incoming_bytes,
            outgoing_bytes,
            topic_sessions,
        } => {
            let peer = device(remote_node_id);
            events.send(NodeEvent::PeerSeen { peer });
            NodeEvent::SyncStarted {
                board,
                peer,
                session_id,
                topic_sessions,
                estimated: SyncCounts {
                    incoming_operations,
                    outgoing_operations,
                    incoming_bytes,
                    outgoing_bytes,
                },
            }
        }
        StreamEvent::SyncEnded {
            remote_node_id,
            session_id,
            sent_operations,
            received_operations,
            sent_bytes,
            received_bytes,
            ref error,
            ..
        } => NodeEvent::SyncEnded {
            board,
            peer: device(remote_node_id),
            session_id,
            transferred: SyncCounts {
                incoming_operations: received_operations,
                outgoing_operations: sent_operations,
                incoming_bytes: received_bytes,
                outgoing_bytes: sent_bytes,
            },
            error: error.as_ref().map(ToString::to_string),
        },
        StreamEvent::ReplayStarted { total_operations } => NodeEvent::ReplayStarted {
            board,
            operations: total_operations,
        },
        StreamEvent::ReplayEnded => NodeEvent::ReplayEnded { board, error: None },
        StreamEvent::ReplayFailed { ref error } => NodeEvent::ReplayEnded {
            board,
            error: Some(error.to_string()),
        },
        _ => return,
    };
    let caught_up = match &status {
        NodeEvent::SyncEnded {
            peer, error: None, ..
        } => Some(*peer),
        _ => None,
    };
    events.send(status);
    if let Some(peer) = caught_up {
        events.send(NodeEvent::CaughtUp { board, peer });
    }
}

fn device(key: NodeId) -> DeviceId {
    DeviceId::from_bytes(*key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2panda_net::discovery::SessionRole;

    #[tokio::test]
    async fn reports_peer_and_session_estimates() {
        let remote = p2panda::Credentials::generate().verifying_key();
        let peer = NodeEvent::PeerSeen {
            peer: device(remote),
        };
        let board = BoardId::from_bytes([27; 32]);
        let tx = Status::default();
        let mut events = tx.subscribe();
        assert!(matches!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::Snapshot(_)
        ));
        let mut active = HashMap::new();
        peer_session(
            &tx,
            &mut active,
            SystemEvent::Discovery(DiscoveryEvent::SessionStarted {
                role: SessionRole::Accepted,
                remote_node_id: remote,
            }),
        );
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::PeerConnected {
                peer: device(remote)
            }
        );
        assert_eq!(events.next().await.unwrap().unwrap(), peer);
        peer_session(
            &tx,
            &mut active,
            SystemEvent::Discovery(DiscoveryEvent::SessionFailed {
                role: SessionRole::Accepted,
                remote_node_id: remote,
                duration: std::time::Duration::from_millis(50),
                reason: "connection closed".into(),
            }),
        );
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::PeerDisconnected {
                peer: device(remote)
            }
        );
        board_status(
            &tx,
            board,
            &StreamEvent::SyncStarted {
                remote_node_id: remote,
                session_id: 2,
                incoming_operations: 12,
                outgoing_operations: 3,
                incoming_bytes: 8192,
                outgoing_bytes: 1024,
                topic_sessions: 1,
            },
        );
        assert_eq!(events.next().await.unwrap().unwrap(), peer);
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::SyncStarted {
                board,
                peer: device(remote),
                session_id: 2,
                topic_sessions: 1,
                estimated: SyncCounts {
                    incoming_operations: 12,
                    outgoing_operations: 3,
                    incoming_bytes: 8192,
                    outgoing_bytes: 1024
                },
            }
        );
    }
}
