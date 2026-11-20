use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::Duration,
};

use pepino_app::{App, LocalState};
use pepino_core::BoardId;
use pepino_node::DeviceCredentials;
use tokio::{sync::watch, task::JoinSet};

use crate::{BoardFailure, Error, Status, Transport, TransportBoard};

pub(crate) struct BoardWorkers<T: Transport> {
    pub node: Arc<T>,
    pub app: Arc<App>,
    pub authority: Option<(std::path::PathBuf, DeviceCredentials)>,
    pub changes: watch::Sender<Status>,
    stopping: watch::Receiver<()>,
    closing: watch::Receiver<()>,
    pub nudged: watch::Receiver<u64>,
    pub poll: Duration,
    pub store_instance: [u8; 16],
    handles: BTreeMap<BoardId, Weak<T::Board>>,
    workers: JoinSet<Result<(), Error>>,
}

impl<T: Transport> BoardWorkers<T> {
    pub fn new(
        node: Arc<T>,
        app: Arc<App>,
        changes: watch::Sender<Status>,
        shutdown: (watch::Receiver<()>, watch::Receiver<()>),
        nudged: watch::Receiver<u64>,
        poll: Duration,
        store_instance: [u8; 16],
    ) -> Self {
        Self {
            node,
            app,
            authority: None,
            changes,
            stopping: shutdown.0,
            closing: shutdown.1,
            nudged,
            poll,
            store_instance,
            handles: BTreeMap::new(),
            workers: JoinSet::new(),
        }
    }

    pub fn handle(&self, id: BoardId) -> Option<Arc<T::Board>> {
        self.handles.get(&id).and_then(Weak::upgrade)
    }

    pub async fn open_missing(&mut self, local: &LocalState) -> Result<(), Error> {
        for board in local.boards() {
            if !self.handles.contains_key(&board.id) {
                let handle = self
                    .node
                    .open_board_for_store(board.id, self.store_instance)
                    .await?;
                self.start_board(local, board.id, handle)?;
            }
        }
        Ok(())
    }

    pub fn start_board(
        &mut self,
        local: &LocalState,
        id: BoardId,
        mut board: T::Board,
    ) -> Result<(), Error> {
        let stream = board.deliveries()?;
        let board = Arc::new(board);
        self.handles.insert(id, Arc::downgrade(&board));
        self.changes.send_modify(|status| {
            let board = status.boards.entry(id).or_default();
            board.enrollment_pending =
                local.created_boards().contains(&id) && local.devices().len() > 1;
            status.board_joins.insert(id, local.board_join_state(id));
            status.node = self.node.status();
        });
        let mut stopping = self.stopping.clone();
        let mut closing = self.closing.clone();
        let app = self.app.clone();
        let status = self.changes.clone();
        let nudged = self.nudged.clone();
        let poll = self.poll;
        let node_instance = self.node.instance_id();
        self.workers.spawn(async move {
            let result = {
                let inbox = crate::inbox::run(&app, id, stream, &status, stopping.clone());
                tokio::pin!(inbox);
                tokio::select! {
                    biased;
                    _ = stopping.changed() => inbox.await,
                    result = &mut inbox => result,
                    _ = crate::outbox::run(&app, id, board.as_ref(), node_instance, &status, nudged, poll) => Err(Error::DeliveryStream("outbox worker ended".into())),
                }
            }
            .map_err(|error| error.to_string());
            let result = result.map_err(|cause| BoardFailure { board: id, cause });
            status.send_modify(|status| {
                let board = status.boards.entry(id).or_default();
                board.closed = true;
                if let Err(failure) = &result {
                    board.last_error = Some(failure.cause.clone());
                    status.restart_required.get_or_insert_with(|| failure.clone());
                }
            });
            if result.is_ok() {
                // Discovery and control requests may still hold a strong board handle.
                let _ = closing.changed().await;
            }
            let closed = match Arc::into_inner(board) {
                Some(board) => board.close().await,
                None => Err(pepino_node::Error::Closed),
            };
            result.map_err(Error::from).and(closed.map_err(Error::from))
        });
        Ok(())
    }

    pub async fn abort(&mut self) {
        self.workers.shutdown().await;
    }

    pub async fn finish(&mut self) -> Result<(), Error> {
        let mut result = Ok(());
        while let Some(worker) = self.workers.join_next().await {
            let outcome = worker.map_err(Error::from).and_then(|outcome| outcome);
            result = result.and(outcome);
        }
        result
    }
}
