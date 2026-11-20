use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::app::{invalid, lock, worker};
use crate::{App, AppError, BoardJoinState, LocalBoard, LocalState};

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

impl LocalState {
    fn name_board(&mut self, id: BoardId, name: &str) {
        let sequence = self
            .catalog
            .revisions
            .get(&id)
            .map_or(1, |previous| previous.sequence.saturating_add(1));
        self.merge_board(
            id,
            Revision {
                sequence,
                writer: self.device_id,
                name: name.into(),
            },
        );
    }

    fn merge_board(&mut self, id: BoardId, revision: Revision) {
        if revision.name.trim().is_empty()
            || self
                .catalog
                .revisions
                .get(&id)
                .is_some_and(|old| old >= &revision)
        {
            return;
        }
        if let Some(board) = self.boards.iter_mut().find(|board| board.id == id) {
            board.name.clone_from(&revision.name);
        } else {
            self.boards.push(LocalBoard {
                id,
                name: revision.name.clone(),
            });
            self.boards[1..].sort_unstable_by_key(|board| board.id);
            self.catalog
                .joins
                .insert(id, BoardJoinState::WaitingToBeAdded);
        }
        self.catalog.revisions.insert(id, revision);
    }
}

impl App {
    pub fn validate_board_name(name: &str) -> Result<(), AppError> {
        if name.trim().is_empty() {
            return Err(invalid(
                "name",
                "board name must contain a non-whitespace character",
            ));
        }
        Ok(())
    }

    pub async fn register_created_board(
        &self,
        id: BoardId,
        name: &str,
    ) -> Result<LocalState, AppError> {
        Self::validate_board_name(name)?;
        let name = name.to_owned();
        self.update_local_state(move |local| {
            if local.catalog.created_here.insert(id) {
                local.name_board(id, &name);
                local.catalog.joins.insert(id, BoardJoinState::Joined);
            }
        })
        .await
    }

    async fn update_local_state(
        &self,
        update: impl FnOnce(&mut LocalState) + Send + 'static,
    ) -> Result<LocalState, AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let directory = self.directory.clone();
        worker("update local state", move || {
            let _guard = guard;
            let previous = LocalState::load(&directory)?;
            let mut local = previous.clone();
            update(&mut local);
            if local != previous {
                local.replace(&directory)?;
            }
            Ok(local)
        })
        .await?
    }
}

#[cfg(test)]
#[test]
fn concurrent_creations_and_renames_join_independent_revisions() {
    use proptest::prelude::*;

    proptest!(|(order in proptest::collection::vec(any::<u8>(), 6))| {
        let anchor = BoardId::from_bytes([40; 32]);
        let workshop = BoardId::from_bytes([51; 32]);
        let greenhouse = BoardId::from_bytes([61; 32]);
        let laptop = DeviceId::from_bytes([71; 32]);
        let tablet = DeviceId::from_bytes([81; 32]);
        let base = LocalState::new(crate::OpenOptions::default().try_into().unwrap(), laptop, anchor).unwrap();
        let mut local = base.clone();
        let revisions = [
            (workshop, 1, laptop, "Workshop"),
            (greenhouse, 1, tablet, "Greenhouse"),
            (workshop, 2, laptop, "Wood Shop"),
            (workshop, 2, tablet, "Tool Shed"),
            (greenhouse, 2, laptop, "Seed House"),
            (workshop, 1, laptop, "Workshop"),
        ];
        let mut operations: Vec<_> = order.into_iter().zip(revisions).collect();
        operations.sort_by_key(|(sort, _)| *sort);
        for (_, (id, sequence, writer, name)) in operations {
            local.merge_board(id, Revision { sequence, writer, name: name.into() });
        }
        prop_assert_eq!(local.board_id(), anchor);
        prop_assert_eq!(local.boards()[1].name.as_str(), "Tool Shed");
        prop_assert_eq!(local.boards()[2].name.as_str(), "Seed House");
        prop_assert_eq!(local.board_join_state(greenhouse), BoardJoinState::WaitingToBeAdded);
    });
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
