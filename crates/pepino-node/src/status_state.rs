use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};

use crate::SyncCounts;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeStatus {
    pub peers_seen: BTreeSet<DeviceId>,
    pub last_seen: BTreeMap<DeviceId, std::time::SystemTime>,
    pub connected_peers: BTreeSet<DeviceId>,
    pub synced_peers: BTreeSet<DeviceId>,
    pub peer_syncs: BTreeMap<DeviceId, u64>,
    pub boards: BTreeMap<BoardId, BoardStatus>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardStatus {
    pub joined: bool,
    pub synced_with_peer: bool,
    pub replaying: bool,
    pub pending_deliveries: usize,
    pub caught_up: BTreeSet<DeviceId>,
    pub active_syncs: BTreeMap<(DeviceId, u64), SyncCounts>,
    pub operation_read_error: Option<String>,
    pub largest_membership_header_item: usize,
    pub membership_header_error: Option<String>,
}

impl BoardStatus {
    /// Past this size the next welcome may exceed what peers can decode.
    pub fn near_header_limit(&self) -> bool {
        self.largest_membership_header_item > crate::HEADER_ITEM_WARNING
    }
}

pub(crate) use tracking::{Status, WeakStatus};

mod tracking {
    use super::*;
    use crate::{NodeEvent, NodeEvents, status};
    use futures_util::{StreamExt, stream};
    use std::sync::{Arc, Mutex, PoisonError, Weak};
    use tokio::sync::broadcast;

    #[derive(Clone)]
    pub(crate) struct Status {
        state: Arc<Mutex<NodeStatus>>,
        changes: broadcast::Sender<NodeEvent>,
    }

    #[derive(Clone)]
    pub(crate) struct WeakStatus {
        state: Weak<Mutex<NodeStatus>>,
        changes: broadcast::WeakSender<NodeEvent>,
    }

    impl WeakStatus {
        pub(crate) fn delivered(&self, board: BoardId) {
            if let (Some(state), Some(changes)) = (self.state.upgrade(), self.changes.upgrade()) {
                Status { state, changes }.pending(board, false);
            }
        }
    }

    impl Default for Status {
        fn default() -> Self {
            Self {
                state: Arc::default(),
                changes: broadcast::channel(128).0,
            }
        }
    }

    impl Status {
        pub(crate) fn snapshot(&self) -> NodeStatus {
            self.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        pub(crate) fn subscribe(&self) -> NodeEvents {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let initial = NodeEvent::Snapshot(state.clone());
            let changes = status::subscribe(&self.changes);
            stream::iter([Ok(initial)]).chain(changes).boxed()
        }
        pub(crate) fn send(&self, event: NodeEvent) {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match &event {
                NodeEvent::PeerConnected { peer } => {
                    state.peers_seen.insert(*peer);
                    state.connected_peers.insert(*peer);
                }
                NodeEvent::PeerSeen { peer } => {
                    state.peers_seen.insert(*peer);
                    state.last_seen.insert(*peer, std::time::SystemTime::now());
                }
                NodeEvent::PeerDisconnected { peer } => {
                    state.connected_peers.remove(peer);
                    for board in state.boards.values_mut() {
                        board.caught_up.remove(peer);
                    }
                }
                NodeEvent::BoardOpened { board, replaying } => {
                    state.boards.insert(
                        *board,
                        BoardStatus {
                            replaying: *replaying,
                            ..Default::default()
                        },
                    );
                }
                NodeEvent::BoardJoined { board } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.joined = true;
                    }
                }
                NodeEvent::BoardClosed { board } => {
                    state.boards.remove(board);
                }
                NodeEvent::ReplayStarted { board, .. } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.replaying = true;
                    }
                }
                NodeEvent::ReplayEnded { board, error } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.replaying = false;
                        if let Some(error) = error {
                            item.operation_read_error =
                                Some(format!("replay board operations: {error}"));
                        }
                    }
                }
                NodeEvent::SyncStarted {
                    board,
                    peer,
                    session_id,
                    estimated,
                    ..
                } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.active_syncs.insert((*peer, *session_id), *estimated);
                        item.caught_up.remove(peer);
                    }
                }
                NodeEvent::SyncEnded {
                    board,
                    peer,
                    session_id,
                    error,
                    ..
                } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.active_syncs.remove(&(*peer, *session_id));
                        if let Some(error) =
                            error.as_ref().filter(|error| is_header_read_error(error))
                        {
                            item.operation_read_error =
                                Some(format!("read a board operation during sync: {error}"));
                        }
                    }
                }
                NodeEvent::MembershipHeaderMeasured {
                    board,
                    largest_item,
                } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        match largest_item {
                            Ok(largest_item) => {
                                item.largest_membership_header_item = *largest_item;
                                item.membership_header_error = None;
                            }
                            Err(error) => item.membership_header_error = Some(error.clone()),
                        }
                    }
                }
                NodeEvent::CaughtUp { board, peer } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.caught_up.insert(*peer);
                        item.synced_with_peer = true;
                        let count = state.peer_syncs.entry(*peer).or_default();
                        *count = count.saturating_add(1);
                        state.synced_peers.insert(*peer);
                        state.peers_seen.insert(*peer);
                        state.last_seen.insert(*peer, std::time::SystemTime::now());
                    }
                }
                _ => (),
            }
            let _ = self.changes.send(event);
        }

        pub(crate) fn downgrade(&self) -> WeakStatus {
            WeakStatus {
                state: Arc::downgrade(&self.state),
                changes: self.changes.downgrade(),
            }
        }

        pub(crate) fn pending(&self, board: BoardId, increase: bool) {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(item) = state.boards.get_mut(&board) {
                if increase {
                    item.pending_deliveries += 1;
                } else {
                    item.pending_deliveries = item.pending_deliveries.saturating_sub(1);
                }
                let _ = self.changes.send(NodeEvent::PendingDeliveries {
                    board,
                    count: item.pending_deliveries,
                });
            }
        }
    }

    // p2panda's public SyncError contains only the protocol error's display text.
    fn is_header_read_error(error: &str) -> bool {
        error.contains("failed decoding incoming header bytes:")
            || error.contains("could not decode corrupted 'header' value")
    }

    #[cfg(test)]
    #[tokio::test]
    async fn late_subscriber_gets_current_state_then_changes() {
        let status = Status::default();
        let board = BoardId::from_bytes([32; 32]);
        let peer = DeviceId::from_bytes([45; 32]);
        status.send(NodeEvent::BoardOpened {
            board,
            replaying: true,
        });
        status.send(NodeEvent::PeerConnected { peer });
        status.pending(board, true);
        let mut events = status.subscribe();
        status.send(NodeEvent::ReplayEnded { board, error: None });
        let NodeEvent::Snapshot(snapshot) = events.next().await.unwrap().unwrap() else {
            panic!("first event must be the snapshot");
        };
        assert!(snapshot.connected_peers.contains(&peer));
        let current = status.snapshot();
        assert!(current.connected_peers.contains(&peer));
        assert!(!current.boards[&board].replaying);
        assert_eq!(current.boards[&board].pending_deliveries, 1);
        assert!(snapshot.boards[&board].replaying);
        assert_eq!(snapshot.boards[&board].pending_deliveries, 1);
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::ReplayEnded { board, error: None }
        );
    }
}

#[cfg(test)]
#[test]
fn reconnect_requires_a_fresh_catch_up() {
    use crate::NodeEvent;

    let status = Status::default();
    let board = BoardId::from_bytes([11; 32]);
    let peer = DeviceId::from_bytes([22; 32]);
    status.send(NodeEvent::BoardOpened {
        board,
        replaying: false,
    });
    assert!(!status.snapshot().boards[&board].joined);
    status.send(NodeEvent::BoardJoined { board });
    status.send(NodeEvent::PeerConnected { peer });
    status.send(NodeEvent::PeerSeen { peer });
    status.send(NodeEvent::CaughtUp { board, peer });
    assert!(status.snapshot().boards[&board].caught_up.contains(&peer));
    status.send(NodeEvent::PeerDisconnected { peer });
    status.send(NodeEvent::PeerConnected { peer });
    let snapshot = status.snapshot();
    assert!(!snapshot.boards[&board].caught_up.contains(&peer));
    assert!(snapshot.synced_peers.contains(&peer));
    assert!(snapshot.last_seen.contains_key(&peer));
    assert!(snapshot.boards[&board].joined && snapshot.boards[&board].synced_with_peer);
    assert!(snapshot.last_seen.contains_key(&peer));
}

#[cfg(test)]
mod operation_read_errors {
    use super::*;
    use crate::NodeEvent;

    const BOARD: BoardId = BoardId::from_bytes([11; 32]);
    const PEER: DeviceId = DeviceId::from_bytes([22; 32]);

    fn after_sync(error: Option<&str>) -> Option<String> {
        let status = Status::default();
        status.send(NodeEvent::BoardOpened {
            board: BOARD,
            replaying: false,
        });
        status.send(sync_ended(error));
        status.snapshot().boards[&BOARD]
            .operation_read_error
            .clone()
    }

    fn sync_ended(error: Option<&str>) -> NodeEvent {
        NodeEvent::SyncEnded {
            board: BOARD,
            peer: PEER,
            session_id: 7,
            transferred: SyncCounts {
                incoming_operations: 3,
                outgoing_operations: 0,
                incoming_bytes: 2_417,
                outgoing_bytes: 0,
            },
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn a_sync_that_cannot_decode_a_header_is_recorded_for_its_board() {
        for cause in [
            "failed decoding incoming header bytes: DecodingHeader(LengthTooLarge)",
            "database error: could not decode corrupted 'header' value",
        ] {
            assert_eq!(
                after_sync(Some(cause)),
                Some(format!("read a board operation during sync: {cause}"))
            );
        }
    }

    #[test]
    fn clean_and_interrupted_syncs_record_no_read_error() {
        assert_eq!(after_sync(None), None);
        assert_eq!(after_sync(Some("connection lost: timed out")), None);
    }

    #[test]
    fn the_read_error_outlives_later_syncs_and_ends_with_the_board() {
        let status = Status::default();
        status.send(NodeEvent::BoardOpened {
            board: BOARD,
            replaying: true,
        });
        status.send(NodeEvent::ReplayEnded {
            board: BOARD,
            error: Some("store: row 14 has no header".into()),
        });
        status.send(sync_ended(None));
        assert_eq!(
            status.snapshot().boards[&BOARD]
                .operation_read_error
                .as_deref(),
            Some("replay board operations: store: row 14 has no header")
        );
        status.send(NodeEvent::BoardClosed { board: BOARD });
        assert!(!status.snapshot().boards.contains_key(&BOARD));
    }
}

#[cfg(test)]
#[test]
fn a_board_nears_the_header_limit_only_past_the_warning_size() {
    use crate::{HEADER_ITEM_WARNING, NodeEvent};

    let status = Status::default();
    let board = BoardId::from_bytes([65; 32]);
    let measured = |largest_item| {
        status.send(NodeEvent::MembershipHeaderMeasured {
            board,
            largest_item: Ok(largest_item),
        });
        status.snapshot().boards.get(&board).cloned()
    };
    assert_eq!(measured(13_000), None);
    status.send(NodeEvent::BoardOpened {
        board,
        replaying: false,
    });
    assert!(!status.snapshot().boards[&board].near_header_limit());
    assert!(!measured(547).unwrap().near_header_limit());
    assert!(!measured(HEADER_ITEM_WARNING).unwrap().near_header_limit());
    assert!(
        measured(HEADER_ITEM_WARNING + 1)
            .unwrap()
            .near_header_limit()
    );
    status.send(NodeEvent::MembershipHeaderMeasured {
        board,
        largest_item: Err("node storage: membership header is not CBOR".into()),
    });
    let unreadable = status.snapshot().boards[&board].clone();
    assert!(unreadable.near_header_limit());
    assert_eq!(
        unreadable.membership_header_error.as_deref(),
        Some("node storage: membership header is not CBOR")
    );
    assert_eq!(measured(547).unwrap().membership_header_error, None);
}
