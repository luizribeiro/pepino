use std::{collections::BTreeMap, time::Duration};

use pepino_app::{App, BoardJoinState};
use pepino_core::BoardId;
use tokio::sync::watch;

use crate::Status;

impl Status {
    fn new_join_progress(&self) -> BTreeMap<BoardId, BoardJoinState> {
        self.board_joins
            .iter()
            .filter_map(|(&id, &previous)| {
                let board = self.boards.get(&id)?;
                let node = self.node.boards.get(&id)?;
                if !node.joined || previous == BoardJoinState::Joined {
                    return None;
                }
                let ready = node.synced_with_peer
                    && !node.replaying
                    && node.pending_deliveries == 0
                    && node.active_syncs.is_empty()
                    && !board.ingest_pending
                    && !board.closed
                    && !board.stream_retrying;
                let observed = if ready {
                    BoardJoinState::Joined
                } else {
                    BoardJoinState::WaitingForFirstSync
                };
                (observed > previous).then_some((id, observed))
            })
            .collect()
    }

    pub fn board_join_states(&self) -> BTreeMap<BoardId, BoardJoinState> {
        self.board_joins
            .iter()
            .map(|(&id, &state)| {
                let state = match self.node.boards.get(&id) {
                    Some(node) if !node.joined => BoardJoinState::WaitingToBeAdded,
                    _ => state,
                };
                (id, state)
            })
            .collect()
    }
}

pub(crate) async fn track(
    app: &App,
    changes: &watch::Sender<Status>,
    mut updates: watch::Receiver<Status>,
) {
    let mut recorded = std::collections::BTreeSet::new();
    loop {
        let (progress, seen) = {
            let status = updates.borrow_and_update();
            let seen = status
                .node
                .synced_peers
                .difference(&recorded)
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            (status.new_join_progress(), seen)
        };
        if !seen.is_empty() {
            if app
                .record_devices_seen_on_boards(seen.clone())
                .await
                .is_err()
            {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            recorded.extend(seen);
        }
        if !progress.is_empty() {
            if app.record_board_joins(progress.clone()).await.is_ok() {
                changes.send_modify(|state| state.board_joins.extend(progress));
            } else {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        }
        if updates.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[test]
fn join_progress_requires_a_peer_and_drained_delivery() {
    use BoardJoinState::*;
    let id = BoardId::from_bytes([42; 32]);
    let mut state = Status {
        board_joins: [(id, WaitingToBeAdded)].into(),
        ..Default::default()
    };
    state.boards.insert(id, Default::default());
    state.node.boards.insert(id, Default::default());
    assert!(state.new_join_progress().is_empty());
    let node = state.node.boards.get_mut(&id).unwrap();
    node.joined = true;
    assert_eq!(state.new_join_progress()[&id], WaitingForFirstSync);
    state.node.boards.get_mut(&id).unwrap().synced_with_peer = true;
    state.boards.get_mut(&id).unwrap().ingest_pending = true;
    assert_eq!(state.new_join_progress()[&id], WaitingForFirstSync);
    state.boards.get_mut(&id).unwrap().ingest_pending = false;
    assert_eq!(state.new_join_progress()[&id], Joined);
}
