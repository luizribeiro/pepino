use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Arc, Mutex, PoisonError},
};

use pepino_core::BoardId;

use crate::Error;

#[derive(Clone, Default)]
pub(crate) struct OpenBoards(Arc<Mutex<HashMap<BoardId, bool>>>);

impl OpenBoards {
    pub(crate) fn reserve(&self, id: BoardId) -> Result<BoardLease, Error> {
        let mut boards = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match boards.entry(id) {
            Entry::Occupied(state) if *state.get() => return Err(Error::BoardOpen(id)),
            Entry::Occupied(_) => return Err(Error::BoardRestartRequired(id)),
            Entry::Vacant(slot) => slot.insert(true),
        };
        Ok(BoardLease {
            id,
            boards: self.clone(),
            attached: false,
        })
    }
}

pub(crate) struct BoardLease {
    pub(crate) id: BoardId,
    boards: OpenBoards,
    attached: bool,
}

impl BoardLease {
    pub(crate) fn attached(mut self) -> Self {
        self.attached = true;
        self
    }
}

impl Drop for BoardLease {
    fn drop(&mut self) {
        let mut boards = self.boards.0.lock().unwrap_or_else(PoisonError::into_inner);
        // In p2panda 0.8.1, closing a space leaves its egress sender registered on the node.
        if self.attached {
            boards.insert(self.id, false);
        } else {
            boards.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_two_active_subscriptions_to_one_board() {
        let open = OpenBoards::default();
        let release = BoardId::from_bytes([32; 32]);
        let support = BoardId::from_bytes([33; 32]);
        let lease = open.reserve(release).unwrap();
        assert!(matches!(open.reserve(release), Err(Error::BoardOpen(id)) if id == release));
        assert!(open.reserve(support).is_ok());
        drop(lease);
        drop(open.reserve(release).unwrap().attached());
        assert!(
            matches!(open.reserve(release), Err(Error::BoardRestartRequired(id)) if id == release)
        );
    }
}
