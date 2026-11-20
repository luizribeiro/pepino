use std::{path::Path, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt};
use pepino_app::{App, OpenOptions};
use pepino_core::{BoardId, DeviceId};
use pepino_node::Node;
use tokio::{sync::watch, task::JoinSet};
use tokio_stream::wrappers::WatchStream;

use crate::{Status, Transport, TransportBoard};

#[derive(Clone, Debug)]
pub struct Settings {
    pub lock_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lock_timeout: OpenOptions::default().lock_timeout,
            poll_interval: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    App(#[from] pepino_app::AppError),
    #[error(transparent)]
    Store(#[from] pepino_store::StoreError),
    #[error(transparent)]
    Node(#[from] pepino_node::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
    #[error("outbox poll interval must be positive")]
    PollInterval,
    #[error("board {0} stopped syncing")]
    BoardStopped(BoardId),
    #[error("sync did not become idle before the deadline")]
    Timeout,
    #[error("data directory belongs to device {local}, but the node is {node}")]
    DeviceMismatch { local: DeviceId, node: DeviceId },
}

pub struct SyncEngine<T: Transport = Node> {
    node: Arc<T>,
    status: watch::Receiver<Status>,
    stop: watch::Sender<()>,
    nudges: watch::Sender<u64>,
    workers: JoinSet<Result<(), Error>>,
}

impl<T: Transport> SyncEngine<T> {
    pub async fn start(
        settings: Settings,
        node: Arc<T>,
        data_dir: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        if settings.poll_interval.is_zero() {
            return Err(Error::PollInterval);
        }
        let options = OpenOptions {
            lock_timeout: settings.lock_timeout,
            ..Default::default()
        };
        let app = Arc::new(App::open_with_options(data_dir, 0, options).await?);
        let local = app.local_state().device_id;
        if local != node.device_id() {
            return Err(Error::DeviceMismatch {
                local,
                node: node.device_id(),
            });
        }
        let store_instance = app.store().instance_id().await?;
        let boards = [app.local_state().board_id];
        let (stop, stopping) = watch::channel(());
        let (nudges, nudged) = watch::channel(0);
        let (changes, status) = watch::channel(Status {
            boards: boards
                .into_iter()
                .map(|board| (board, Default::default()))
                .collect(),
            node: node.status(),
            ..Default::default()
        });
        let mut workers = JoinSet::new();
        let node_instance = node.instance_id();
        for id in boards {
            let mut board = node.open_board_for_store(id, store_instance).await?;
            let stream = board.deliveries()?;
            let mut stopping = stopping.clone();
            let app = app.clone();
            let status = changes.clone();
            let nudged = nudged.clone();
            let poll = settings.poll_interval;
            workers.spawn(async move {
                let result = tokio::select! {
                    biased;
                    _ = stopping.changed() => Ok(()),
                    _ = crate::outbox::run(&app, id, &board, node_instance, &status, nudged, poll) => Ok(()),
                    result = crate::inbox::run(&app, id, stream, &status) => result,
                };
                status.send_modify(|status| {
                    let board = status.boards.entry(id).or_default();
                    board.closed = true;
                    if let Err(error) = &result {
                        board.last_error = Some(error.to_string());
                    }
                });
                result.and(board.close().await.map_err(Error::from))
            });
        }
        changes.send_modify(|status| status.node = node.status());
        let observer = node.clone();
        workers.spawn(async move {
            observe_node(observer.as_ref(), changes, stopping).await;
            Ok(())
        });
        Ok(Self {
            node,
            status,
            stop,
            nudges,
            workers,
        })
    }

    /// Request a scan after a local write commits.
    pub fn nudge(&self) {
        self.request_scan();
    }

    fn request_scan(&self) -> u64 {
        let mut requested = 0;
        self.nudges.send_modify(|generation| {
            *generation = generation.saturating_add(1);
            requested = *generation;
        });
        requested
    }

    /// Scan for new writes and wait until all known deliveries are acknowledged.
    pub async fn sync_once(&self) -> Result<(), Error> {
        let generation = self.request_scan();
        let mut updates = self.status.clone();
        loop {
            {
                let status = updates.borrow_and_update();
                if let Some((id, _)) = status.boards.iter().find(|(_, board)| board.closed) {
                    return Err(Error::BoardStopped(*id));
                }
                if status
                    .boards
                    .values()
                    .all(|board| board.scan_generation >= Some(generation))
                    && status.idle_with(&self.node.status())
                {
                    return Ok(());
                }
            }
            updates
                .changed()
                .await
                .map_err(|_| pepino_node::Error::Closed)?;
        }
    }

    /// Wait for local idle with a deadline. Peers can send more work later.
    pub async fn wait_idle(&self, timeout: Duration) -> Result<(), Error> {
        tokio::time::timeout(timeout, self.sync_once())
            .await
            .map_err(|_| Error::Timeout)?
    }

    pub fn status(&self) -> Status {
        let status = self.status.borrow();
        Status {
            node: self.node.status(),
            ..status.clone()
        }
    }

    /// Emits the current snapshot followed by changes. Slow consumers may skip snapshots.
    pub fn status_changes(&self) -> impl Stream<Item = Status> + use<T> {
        WatchStream::new(self.status.clone())
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self {
            stop, mut workers, ..
        } = self;
        drop(stop);
        let mut result = Ok(());
        while let Some(worker) = workers.join_next().await {
            let outcome = worker.map_err(Error::from).and_then(|outcome| outcome);
            result = result.and(outcome);
        }
        result
    }
}

async fn observe_node<T: Transport>(
    node: &T,
    changes: watch::Sender<Status>,
    mut stopping: watch::Receiver<()>,
) {
    let mut events = node.events();
    loop {
        tokio::select! {
            biased;
            _ = stopping.changed() => return,
            event = events.next() => {
                let Some(event) = event else { return };
                let snapshot = node.status();
                changes.send_modify(|status| {
                    status.node = snapshot;
                    match event {
                        Ok(event) => status.last_node_event = Some(event),
                        Err(error) => status.last_node_error = Some(error.to_string()),
                    }
                });
            }
        }
    }
}
