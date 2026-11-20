use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};

use crate::SyncCounts;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeStatus {
    pub connected_peers: BTreeSet<DeviceId>,
    pub boards: BTreeMap<BoardId, BoardStatus>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardStatus {
    pub replaying: bool,
    pub pending_deliveries: usize,
    pub caught_up: BTreeSet<DeviceId>,
    pub active_syncs: BTreeMap<(DeviceId, u64), SyncCounts>,
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
                    state.connected_peers.insert(*peer);
                }
                NodeEvent::PeerDisconnected { peer } => {
                    state.connected_peers.remove(peer);
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
                NodeEvent::BoardClosed { board } => {
                    state.boards.remove(board);
                }
                NodeEvent::ReplayStarted { board, .. } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.replaying = true;
                    }
                }
                NodeEvent::ReplayEnded { board, .. } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.replaying = false;
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
                    ..
                } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.active_syncs.remove(&(*peer, *session_id));
                    }
                }
                NodeEvent::CaughtUp { board, peer } => {
                    if let Some(item) = state.boards.get_mut(board) {
                        item.caught_up.insert(*peer);
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
        assert!(snapshot.boards[&board].replaying);
        assert_eq!(snapshot.boards[&board].pending_deliveries, 1);
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::ReplayEnded { board, error: None }
        );
    }
}
