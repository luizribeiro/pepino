use std::collections::{BTreeMap, BTreeSet};

use pepino_app::BoardJoinState;
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{NodeEvent, NodeStatus};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    pub newer_catalog: Option<u32>,
    pub board_joins: BTreeMap<BoardId, BoardJoinState>,
    pub(crate) catalog_generation: u64,
    pub(crate) roster_pending: Option<BoardId>,
    pub members: BTreeMap<BoardId, BTreeSet<DeviceId>>,
    pub boards: BTreeMap<BoardId, BoardStatus>,
    pub device_removals: BTreeMap<DeviceId, Vec<crate::BoardRemoval>>,
    pub node: NodeStatus,
    pub last_node_event: Option<NodeEvent>,
    pub last_node_error: Option<String>,
    /// A terminal worker failure. The daemon must shut down the engine and node,
    /// then start both again.
    pub restart_required: Option<BoardFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("board {board} stopped syncing: {cause}. Restart the engine and node")]
pub struct BoardFailure {
    pub board: BoardId,
    pub cause: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardStatus {
    pub(crate) paused: bool,
    /// Unknown until the first successful scan.
    pub outbox_pending: Option<u64>,
    pub ingest_pending: bool,
    pub stream_retrying: bool,
    pub published: u64,
    pub received: u64,
    pub last_published_at: Option<std::time::SystemTime>,
    pub last_received_at: Option<std::time::SystemTime>,
    pub last_published: Option<EventId>,
    pub last_ingested: Option<EventId>,
    pub last_error: Option<String>,
    pub closed: bool,
    pub(crate) scan_generation: Option<u64>,
    pub(crate) enrollment_pending: bool,
}

impl BoardStatus {
    pub fn enrollment_pending(&self) -> bool {
        self.enrollment_pending
    }
}

impl Status {
    /// Reports whether local work has drained and every connected peer is caught up.
    /// Without connected peers, only local work is considered.
    pub fn is_idle(&self) -> bool {
        self.idle_with(&self.node)
    }

    pub fn board_caught_up(&self, board: BoardId, node: &NodeStatus) -> bool {
        node.boards.get(&board).is_some_and(|signals| {
            node.connected_peers.iter().all(|peer| {
                self.members
                    .get(&board)
                    .is_some_and(|members| !members.contains(peer))
                    || signals.caught_up.contains(peer)
            })
        })
    }

    pub(crate) fn idle_with(&self, node: &NodeStatus) -> bool {
        self.restart_required.is_none()
            && self.boards.iter().all(|(id, board)| {
                let signals = node.boards.get(id);
                if self.board_joins.get(id) == Some(&BoardJoinState::WaitingToBeAdded)
                    && let Some(signals) = signals.filter(|signals| !signals.joined)
                {
                    return !board.ingest_pending
                        && !board.stream_retrying
                        && !signals.replaying
                        && signals.active_syncs.is_empty()
                        && signals.pending_deliveries == 0;
                }
                board.paused
                    || (!board.closed
                        && board.outbox_pending == Some(0)
                        && !board.ingest_pending
                        && !board.stream_retrying
                        && node.boards.get(id).is_some_and(|signals| {
                            !signals.replaying
                                && signals.active_syncs.is_empty()
                                && signals.pending_deliveries == 0
                                && self.board_caught_up(*id, node)
                        }))
            })
    }
}

pub(crate) fn record_error(
    status: &tokio::sync::watch::Sender<Status>,
    board: BoardId,
    error: String,
) {
    status.send_if_modified(|state| {
        let board = state.boards.entry(board).or_default();
        if board.last_error.as_ref() == Some(&error) {
            return false;
        }
        board.last_error = Some(error);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_requires_a_scan_and_no_queued_or_active_delivery() {
        let board = BoardId::from_bytes([23; 32]);
        let mut status = Status::default();
        status.boards.insert(board, BoardStatus::default());
        status.node.boards.insert(board, Default::default());
        assert!(!status.is_idle());
        status.boards.get_mut(&board).unwrap().outbox_pending = Some(0);
        assert!(status.is_idle());
        status
            .node
            .boards
            .get_mut(&board)
            .unwrap()
            .pending_deliveries = 2;
        assert!(!status.is_idle());
        status
            .node
            .boards
            .get_mut(&board)
            .unwrap()
            .pending_deliveries = 0;
        status.boards.get_mut(&board).unwrap().ingest_pending = true;
        assert!(!status.is_idle());
    }
}

#[cfg(test)]
#[test]
fn a_repeated_error_preserves_the_snapshot_without_waking_observers() {
    let board = BoardId::from_bytes([12; 32]);
    let (status, mut updates) = tokio::sync::watch::channel(Status::default());
    let message = String::from("the data directory is locked");
    record_error(&status, board, message.clone());
    assert!(updates.has_changed().unwrap());
    assert_eq!(
        updates.borrow_and_update().boards[&board].last_error,
        Some(message.clone())
    );
    record_error(&status, board, message);
    assert!(!updates.has_changed().unwrap());
}

#[cfg(test)]
#[test]
fn an_unrelated_connected_peer_does_not_block_private_board_idle() {
    let board = BoardId::from_bytes([34; 32]);
    let stranger = DeviceId::from_bytes([35; 32]);
    let member = DeviceId::from_bytes([36; 32]);
    let mut status = Status::default();
    status.board_joins.insert(board, BoardJoinState::Joined);
    status.boards.entry(board).or_default().outbox_pending = Some(0);
    status.node.boards.insert(board, Default::default());
    status.members.insert(board, [member].into());
    status.node.connected_peers.insert(stranger);
    assert!(status.is_idle());
    status.node.connected_peers.insert(member);
    assert!(!status.is_idle());
}

#[cfg(test)]
#[test]
fn waiting_for_access_is_idle_with_connected_peers() {
    let board = BoardId::from_bytes([42; 32]);
    let peer = DeviceId::from_bytes([43; 32]);
    let mut status = Status {
        board_joins: [(board, BoardJoinState::WaitingToBeAdded)].into(),
        ..Default::default()
    };
    status.boards.insert(
        board,
        BoardStatus {
            outbox_pending: Some(0),
            ..Default::default()
        },
    );
    status.node.boards.insert(board, Default::default());
    status.node.connected_peers.insert(peer);
    assert!(!status.board_caught_up(board, &status.node));
    assert!(status.is_idle());
    status.boards.get_mut(&board).unwrap().ingest_pending = true;
    assert!(!status.is_idle());
    status.boards.get_mut(&board).unwrap().ingest_pending = false;
    status
        .node
        .boards
        .get_mut(&board)
        .unwrap()
        .pending_deliveries = 3;
    assert!(!status.is_idle());
    let signals = status.node.boards.get_mut(&board).unwrap();
    signals.pending_deliveries = 0;
    signals.active_syncs.insert(
        (peer, 1),
        pepino_node::SyncCounts {
            incoming_operations: 3,
            outgoing_operations: 0,
            incoming_bytes: 900,
            outgoing_bytes: 0,
        },
    );
    assert!(!status.is_idle());
    status
        .node
        .boards
        .get_mut(&board)
        .unwrap()
        .active_syncs
        .clear();
    assert!(status.is_idle());
    status.node.boards.get_mut(&board).unwrap().joined = true;
    assert!(!status.is_idle());
}
