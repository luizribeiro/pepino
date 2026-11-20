use std::collections::BTreeMap;

use pepino_core::{BoardId, EventId};
use pepino_node::{NodeEvent, NodeStatus};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    pub boards: BTreeMap<BoardId, BoardStatus>,
    pub node: NodeStatus,
    pub last_node_event: Option<NodeEvent>,
    pub last_node_error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardStatus {
    /// Unknown until the first successful scan.
    pub outbox_pending: Option<u64>,
    pub ingest_pending: bool,
    pub last_published: Option<EventId>,
    pub last_ingested: Option<EventId>,
    pub last_error: Option<String>,
    pub closed: bool,
    pub(crate) scan_generation: Option<u64>,
}

impl Status {
    /// Reports whether local work has drained and every connected peer is caught up.
    /// Without connected peers, only local work is considered.
    pub fn is_idle(&self) -> bool {
        self.idle_with(&self.node)
    }

    pub(crate) fn idle_with(&self, node: &NodeStatus) -> bool {
        self.boards.iter().all(|(id, board)| {
            !board.closed
                && board.outbox_pending == Some(0)
                && !board.ingest_pending
                && node.boards.get(id).is_some_and(|signals| {
                    !signals.replaying
                        && signals.active_syncs.is_empty()
                        && signals.pending_deliveries == 0
                        && node.connected_peers.is_subset(&signals.caught_up)
                })
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
