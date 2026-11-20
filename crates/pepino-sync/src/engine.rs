use std::{path::Path, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt};
use pepino_app::{App, OpenOptions};
use pepino_core::DeviceId;
use pepino_node::Node;
use tokio::{sync::watch, task::JoinSet};
use tokio_stream::wrappers::WatchStream;

use crate::{Status, Transport, TransportBoard};

#[derive(Clone, Debug)]
pub struct Settings {
    pub lock_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lock_timeout: OpenOptions::default().lock_timeout,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    App(#[from] pepino_app::AppError),
    #[error(transparent)]
    Node(#[from] pepino_node::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
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
        for id in boards {
            let mut board = node.open_board(id).await?;
            let stream = board.deliveries()?;
            let mut stopping = stopping.clone();
            let app = app.clone();
            let status = changes.clone();
            let nudged = nudged.clone();
            workers.spawn(async move {
                let result = tokio::select! {
                    biased;
                    _ = stopping.changed() => Ok(()),
                    _ = crate::outbox::run(&app, id, &board, &status, nudged) => Ok(()),
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
        self.nudges
            .send_modify(|generation| *generation = generation.saturating_add(1));
    }

    pub fn status(&self) -> Status {
        let mut status = self.status.borrow().clone();
        status.node = self.node.status();
        status
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
