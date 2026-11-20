use std::{path::Path, sync::Arc, time::Duration};

use pepino_app::{App, OpenOptions};
use pepino_core::DeviceId;
use pepino_node::Node;
use tokio::{sync::watch, task::JoinSet};

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
        let app = App::open_with_options(data_dir, 0, options).await?;
        let local = app.local_state().device_id;
        if local != node.device_id() {
            return Err(Error::DeviceMismatch {
                local,
                node: node.device_id(),
            });
        }
        let boards = [app.local_state().board_id];
        let (stop, stopping) = watch::channel(());
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
            let board = node.open_board(id).await?;
            let mut stopping = stopping.clone();
            workers.spawn(async move {
                let _ = stopping.changed().await;
                Ok(board.close().await?)
            });
        }
        changes.send_modify(|status| status.node = node.status());
        Ok(Self {
            node,
            status,
            stop,
            workers,
        })
    }

    pub fn status(&self) -> Status {
        let mut status = self.status.borrow().clone();
        status.node = self.node.status();
        status
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
