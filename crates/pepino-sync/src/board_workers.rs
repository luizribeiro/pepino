mod card_exchange;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    time::Duration,
};

use pepino_app::{App, LocalState};
use pepino_core::BoardId;
use pepino_node::DeviceCredentials;
use tokio::{
    sync::{oneshot, watch},
    task::JoinSet,
};

use crate::{BoardFailure, DeliveryStream, Error, Status, Transport, TransportBoard};

pub(crate) fn should_open(local: &LocalState, board: BoardId) -> bool {
    !local.board_needs_transport_purge(board)
        && (!local.board_removal_confirmed(board) || local.needs_readmission_replay(board))
}

struct WorkerStop {
    evict: oneshot::Sender<()>,
    closed: oneshot::Receiver<()>,
}

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
    board_workers: BTreeMap<BoardId, WorkerStop>,
    card_workers: BTreeMap<BoardId, WorkerStop>,
    workers: JoinSet<Result<(), Error>>,
    pub(crate) replaying: std::collections::BTreeSet<BoardId>,
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
            board_workers: BTreeMap::new(),
            card_workers: BTreeMap::new(),
            workers: JoinSet::new(),
            replaying: Default::default(),
        }
    }

    pub fn handle(&self, id: BoardId) -> Option<Arc<T::Board>> {
        self.handles.get(&id).and_then(Weak::upgrade)
    }

    pub(crate) async fn close_forgotten(&mut self, local: &LocalState) -> bool {
        let absent = |id| !local.boards().iter().any(|board| board.id == id);
        let node = self.node.status();
        if let Some(&board) = self.board_workers.keys().find(|&&board| {
            absent(board)
                && !node
                    .boards
                    .get(&board)
                    .is_some_and(|signals| signals.joined)
        }) {
            self.changes.send_modify(|status| {
                require_restart(
                    status,
                    board,
                    "an unjoined board was removed from the catalog",
                );
            });
            return true;
        }
        for (board, worker) in self.board_workers.extract_if(.., |board, _| absent(*board)) {
            self.handles.remove(&board);
            self.replaying.remove(&board);
            let _ = worker.evict.send(());
            let _ = worker.closed.await;
            self.changes.send_modify(|status| {
                status.boards.remove(&board);
                status.board_joins.remove(&board);
                status.members.remove(&board);
            });
        }
        self.changes.borrow().restart_required.is_some()
    }

    pub async fn open_missing(&mut self, local: &LocalState) -> Result<(), Error> {
        for board in local.boards() {
            if self.stopping.has_changed().is_err() {
                return Ok(());
            }
            if local.board_needs_transport_purge(board.id) {
                self.changes.send_modify(|status| {
                    require_restart(
                        status,
                        board.id,
                        "retry the purge before reopening the board",
                    );
                });
                return Ok(());
            }
            if should_open(local, board.id) && !self.handles.contains_key(&board.id) {
                let replay = local.needs_readmission_replay(board.id);
                let opening = if replay {
                    self.node.open_board_for_readmission(board.id).await
                } else {
                    self.node
                        .open_board_for_store(board.id, self.store_instance)
                        .await
                };
                let handle = match opening {
                    Ok(handle) => handle,
                    Err(pepino_node::Error::BoardRestartRequired(_)) => {
                        self.changes.send_modify(|status| {
                            require_restart(
                                status,
                                board.id,
                                "reopen a board after leaving locally",
                            );
                        });
                        return Ok(());
                    }
                    Err(error) => return Err(error.into()),
                };
                if replay && !handle.readmission_replay_deferred() {
                    self.replaying.insert(board.id);
                }
                self.start_board(local, board.id, handle)?;
            }
        }
        self.open_card_exchanges(local).await
    }

    pub fn start_board(
        &mut self,
        local: &LocalState,
        id: BoardId,
        mut board: T::Board,
    ) -> Result<(), Error> {
        let stream = board.deliveries()?;
        if local.catalog_board() == id
            && let Some(membership) = board.membership_events()
        {
            let mut stopping = self.stopping.clone();
            let app = self.app.clone();
            self.workers.spawn(async move {
                tokio::select! {
                    _ = stopping.changed() => (),
                    _ = crate::removal_notice::observe(&app, membership) => (),
                }
                Ok(())
            });
        }
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
        let authority = self.authority.as_ref().map(|(path, _)| path.clone());
        let (evict, mut evicted) = oneshot::channel();
        let (finished, closed) = oneshot::channel();
        self.board_workers.insert(id, WorkerStop { evict, closed });
        self.workers.spawn(async move {
            let result = {
                let inbox = receive(&app, id, board.as_ref(), stream, &status, stopping.clone(), authority.as_deref());
                tokio::pin!(inbox);
                tokio::select! {
                    biased;
                    _ = &mut evicted => None,
                    _ = stopping.changed() => Some(inbox.await),
                    result = &mut inbox => Some(result),
                    _ = crate::outbox::run(&app, id, board.as_ref(), node_instance, &status, nudged, poll) => Some(Err(Error::DeliveryStream("outbox worker ended".into()))),
                }
            };
            let result = match result {
                Some(result) => {
                    let result = result.map_err(|error| BoardFailure { board: id, cause: error.to_string() });
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
                        tokio::select! {
                            _ = closing.changed() => Some(result),
                            _ = &mut evicted => None,
                        }
                    } else {
                        Some(result)
                    }
                }
                None => None,
            };
            let closed = close_board(board).await;
            match result {
                None => {
                    if let Err(error) = closed {
                        status.send_modify(|status| require_restart(status, id, &error.to_string()));
                    }
                    let _ = finished.send(());
                    Ok(())
                }
                Some(result) => result.map_err(Error::from).and(closed.map_err(Error::from)),
            }
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

fn require_restart(status: &mut Status, board: BoardId, cause: &str) {
    status.restart_required.get_or_insert_with(|| BoardFailure {
        board,
        cause: cause.into(),
    });
}

async fn close_board(board: Arc<impl TransportBoard>) -> Result<(), pepino_node::Error> {
    match Arc::into_inner(board) {
        Some(board) => board.close().await,
        None => Err(pepino_node::Error::Closed),
    }
}

async fn receive<B: TransportBoard>(
    app: &App,
    id: BoardId,
    board: &B,
    stream: DeliveryStream<B::Ack>,
    status: &watch::Sender<Status>,
    mut stopping: watch::Receiver<()>,
    authority: Option<&std::path::Path>,
) -> Result<(), Error> {
    use futures_util::StreamExt;
    use pepino_node::{Error as NodeError, MemberState};
    let mut membership = board.membership_events();
    match crate::inbox::run(app, id, board, stream, status, stopping.clone(), authority).await {
        Err(Error::Node(NodeError::Removed)) => {
            if id != app.local_state().catalog_board() {
                app.observe_board_removal(id, true).await?;
            }
        }
        result => return result,
    }
    let Some(changes) = &mut membership else {
        return Ok(());
    };
    status.send_modify(|status| status.boards.entry(id).or_default().stream_retrying = false);
    loop {
        let local = app.local_state();
        let active = match board.known_members(local, false).await {
            Ok(members) => members.is_some_and(|members| members.contains(&local.device_id)),
            Err(NodeError::NotJoined) => false,
            Err(error) => return Err(error.into()),
        };
        if active {
            app.observe_board_removal(id, false).await?;
            app.record_readmission_replay(id, true).await?;
            return Err(Error::DeliveryStream("replay after readmission".into()));
        }
        tokio::select! {
            _ = stopping.changed() => return Ok(()),
            event = changes.next() => match event {
                None => return Ok(()),
                Some(Ok(event)) if matches!(event.members.get(&app.local_state().device_id), Some(MemberState::Member(_))) => (),
                _ => continue,
            },
        }
    }
}
