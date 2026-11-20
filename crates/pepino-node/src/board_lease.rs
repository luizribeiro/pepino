use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use pepino_core::BoardId;
use tokio::sync::{Notify, watch};

use crate::Error;

#[derive(Clone, Default)]
pub(crate) struct OpenBoards(Arc<Registry>);

#[derive(Default)]
struct Registry {
    leases: Mutex<Leases>,
    stopping: watch::Sender<bool>,
    released: Notify,
    preparing: AtomicBool,
    retained: Mutex<Vec<Box<dyn Send>>>,
}

#[derive(Default)]
struct Leases {
    boards: HashMap<BoardId, bool>,
    creating: usize,
}

impl OpenBoards {
    fn check_open(&self) -> Result<(), Error> {
        if self.0.preparing.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        Ok(())
    }

    pub(crate) fn begin_creation(&self) -> Result<BoardCreation, Error> {
        let mut leases = self.0.leases.lock().unwrap_or_else(PoisonError::into_inner);
        self.check_open()?;
        leases.creating += 1;
        Ok(BoardCreation {
            boards: self.clone(),
        })
    }

    pub(crate) fn reserve(&self, id: BoardId) -> Result<BoardLease, Error> {
        self.reserve_with(id, || self.check_open())
    }

    fn reserve_with(
        &self,
        id: BoardId,
        check: impl FnOnce() -> Result<(), Error>,
    ) -> Result<BoardLease, Error> {
        let mut leases = self.0.leases.lock().unwrap_or_else(PoisonError::into_inner);
        check()?;
        match leases.boards.entry(id) {
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

    pub(crate) fn prepare_shutdown(&self) {
        self.0.preparing.store(true, Ordering::Release);
    }

    pub(crate) fn release_upstream(&self) {
        let retained = std::mem::take(
            &mut *self
                .0
                .retained
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        drop(retained);
    }

    pub(crate) async fn stop(&self) -> Result<(), Error> {
        self.prepare_shutdown();
        self.0.stopping.send_replace(true);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.drained() {
                    return;
                }
                self.0.released.notified().await;
            }
        })
        .await
        .map_err(|_| Error::Worker("timed out closing board subscriptions".into()))
    }
    fn drained(&self) -> bool {
        let leases = self.0.leases.lock().unwrap_or_else(PoisonError::into_inner);
        leases.creating == 0 && !leases.boards.values().any(|active| *active)
    }

    fn retain_upstream(&self, handles: impl Send + 'static) {
        if self.0.preparing.load(Ordering::Acquire) {
            self.0
                .retained
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Box::new(handles));
        }
    }
}

pub(crate) struct BoardCreation {
    boards: OpenBoards,
}

impl BoardCreation {
    pub(crate) fn check_open(&self) -> Result<(), Error> {
        self.boards.check_open()
    }

    pub(crate) fn register(&self, id: BoardId) -> Result<BoardLease, Error> {
        self.boards.reserve_with(id, || Ok(()))
    }

    pub(crate) fn retain_upstream(&self, handles: impl Send + 'static) {
        self.boards.retain_upstream(handles);
    }
}

impl Drop for BoardCreation {
    fn drop(&mut self) {
        let registry = &self.boards.0;
        let mut leases = registry
            .leases
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        leases.creating -= 1;
        registry.released.notify_one();
    }
}

pub(crate) struct BoardLease {
    pub(crate) id: BoardId,
    boards: OpenBoards,
    attached: bool,
}

impl BoardLease {
    pub(crate) fn check_open(&self) -> Result<(), Error> {
        self.boards.check_open()
    }

    pub(crate) fn stopping(&self) -> watch::Receiver<bool> {
        self.boards.0.stopping.subscribe()
    }

    pub(crate) fn retain_upstream(&self, handles: impl Send + 'static) {
        self.boards.retain_upstream(handles);
    }

    pub(crate) fn attached(mut self) -> Self {
        self.attached = true;
        self
    }
}

impl Drop for BoardLease {
    fn drop(&mut self) {
        let mut leases = self
            .boards
            .0
            .leases
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // In p2panda 0.8.1, closing a space leaves its egress sender registered on the node.
        if self.attached {
            leases.boards.insert(self.id, false);
        } else {
            leases.boards.remove(&self.id);
        }
        self.boards.0.released.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_rejects_new_boards_and_waits_for_the_last_lease() {
        let open = OpenBoards::default();
        let release = BoardId::from_bytes([32; 32]);
        let support = BoardId::from_bytes([33; 32]);
        let lease = open.reserve(release).unwrap().attached();
        let mut stopping = lease.stopping();
        let closing = async {
            stopping.wait_for(|value| *value).await.unwrap();
            assert!(matches!(open.reserve(support), Err(Error::Closed)));
            assert!(matches!(lease.check_open(), Err(Error::Closed)));
            drop(lease);
        };
        let (result, ()) = tokio::join!(open.stop(), closing);
        result.unwrap();
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_creation_to_transfer_its_board_lease() {
        let open = OpenBoards::default();
        let creation = open.begin_creation().unwrap();
        open.prepare_shutdown();
        assert!(matches!(open.begin_creation(), Err(Error::Closed)));
        let stopped = open.stop();
        tokio::pin!(stopped);
        assert!(matches!(
            futures_util::poll!(&mut stopped),
            std::task::Poll::Pending
        ));
        let board = creation
            .register(BoardId::from_bytes([37; 32]))
            .unwrap()
            .attached();
        drop(creation);
        assert!(matches!(
            futures_util::poll!(&mut stopped),
            std::task::Poll::Pending
        ));
        drop(board);
        stopped.await.unwrap();
    }

    #[tokio::test]
    async fn upstream_handles_outlive_the_board_lease_during_node_shutdown() {
        let open = OpenBoards::default();
        let lease = open.reserve(BoardId::from_bytes([34; 32])).unwrap();
        let marker = Arc::new("greenhouse subscription");
        open.prepare_shutdown();
        lease.retain_upstream(marker.clone());
        drop(lease);
        open.stop().await.unwrap();
        assert_eq!(Arc::strong_count(&marker), 2);
        open.release_upstream();
        assert_eq!(Arc::strong_count(&marker), 1);
    }

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
