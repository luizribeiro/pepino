use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::BoardJoinState;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct CatalogState {
    revisions: BTreeMap<BoardId, Revision>,
    joins: BTreeMap<BoardId, BoardJoinState>,
    created_here: BTreeSet<BoardId>,
    bundles: BTreeMap<DeviceId, String>,
}

impl CatalogState {
    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub(crate) fn join(&self, id: BoardId) -> Option<BoardJoinState> {
        self.joins.get(&id).copied()
    }

    pub(crate) fn record_joins(&mut self, progress: &BTreeMap<BoardId, BoardJoinState>) {
        for (id, state) in progress {
            if let Some(previous) = self.joins.get_mut(id) {
                *previous = (*previous).max(*state);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
struct Revision {
    sequence: u64,
    writer: DeviceId,
    name: String,
}

#[cfg(test)]
#[test]
fn board_join_progress_is_monotonic() {
    let id = BoardId::from_bytes([64; 32]);
    let mut state = CatalogState::default();
    state.joins.insert(id, BoardJoinState::WaitingToBeAdded);
    state.record_joins(&[(id, BoardJoinState::Joined)].into());
    state.record_joins(&[(id, BoardJoinState::WaitingForFirstSync)].into());
    assert_eq!(state.join(id), Some(BoardJoinState::Joined));
}
