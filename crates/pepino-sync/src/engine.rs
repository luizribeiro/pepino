use std::{path::Path, sync::Arc, time::Duration};

use futures_util::{Stream, StreamExt};
use pepino_app::{App, DeviceRequest, OpenOptions, PeerHint};
use pepino_core::{BoardId, DeviceId};
use pepino_node::{Node, Role};
use tokio::{
    sync::{Mutex, watch},
    task::JoinSet,
};
use tokio_stream::wrappers::WatchStream;

use crate::{BoardFailure, Status, Transport, board_workers::BoardWorkers};

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
    #[error(transparent)]
    RestartRequired(#[from] BoardFailure),
    #[error("delivery stream ended: {0}")]
    DeliveryStream(String),
    #[error("This device needs manage access to board {board:?} before adding a device.")]
    EnrollmentAccess { board: String },
    #[error("Some boards could not enroll the device")]
    EnrollmentIncomplete {
        enrolled: Vec<String>,
        failed: Vec<String>,
    },
    #[error("sync did not become idle before the deadline")]
    Timeout,
    #[error("data directory belongs to device {local}, but the node is {node}")]
    DeviceMismatch { local: DeviceId, node: DeviceId },
}

/// Watch [`Status::restart_required`] through [`Self::status_changes`] to supervise
/// the engine. On failure, the daemon must shut down both the engine and the
/// node, then start both again.
pub struct SyncEngine<T: Transport = Node> {
    node: Arc<T>,
    app: Arc<App>,
    boards: Arc<Mutex<BoardWorkers<T>>>,
    status: watch::Receiver<Status>,
    stop: watch::Sender<()>,
    board_stop: watch::Sender<()>,
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
        let (stop, stopping) = watch::channel(());
        let (board_stop, boards_stopping) = watch::channel(());
        let (nudges, nudged) = watch::channel(0);
        let (changes, status) = watch::channel(Status {
            node: node.status(),
            ..Default::default()
        });
        let mut workers = JoinSet::new();
        let mut board_workers = BoardWorkers::new(
            node.clone(),
            app.clone(),
            changes.clone(),
            boards_stopping,
            nudged,
            settings.poll_interval,
            store_instance,
        );
        board_workers.open_missing(app.local_state()).await?;
        let boards = Arc::new(Mutex::new(board_workers));
        workers.spawn(crate::discover::run(boards.clone(), stopping.clone()));
        let progress_app = app.clone();
        let progress_changes = changes.clone();
        let progress_updates = status.clone();
        let mut progress_stop = stopping.clone();
        workers.spawn(async move {
            tokio::select! {
                _ = progress_stop.changed() => (),
                _ = crate::join_progress::track(&progress_app, &progress_changes, progress_updates) => (),
            }
            Ok(())
        });
        let observer = node.clone();
        workers.spawn(async move {
            observe_node(observer.as_ref(), changes, stopping).await;
            Ok(())
        });
        Ok(Self {
            node,
            app,
            boards,
            status,
            stop,
            board_stop,
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
                if let Some(failure) = &status.restart_required {
                    return Err(failure.clone().into());
                }
                if status.catalog_generation >= generation
                    && status
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
            stop,
            board_stop,
            mut workers,
            boards,
            ..
        } = self;
        drop(stop);
        let mut result = Ok(());
        while let Some(worker) = workers.join_next().await {
            let outcome = worker.map_err(Error::from).and_then(|outcome| outcome);
            result = result.and(outcome);
        }
        drop(board_stop);
        result.and(boards.lock().await.finish().await)
    }
}

impl SyncEngine {
    pub async fn add_member(
        &self,
        device: DeviceRequest,
        name: Option<String>,
    ) -> Result<(), Error> {
        let member = device.member_info()?;
        if device.id == self.node.device_id()
            || name.as_deref().is_some_and(|name| name.trim().is_empty())
        {
            return Err(Error::App(pepino_app::AppError::InvalidInput {
                field: "device".into(),
                reason: "expected a different device and a non-blank name".into(),
                diagnostic: None,
            }));
        }
        let mut boards = self.boards.lock().await;
        let local = self.app.fresh_local_state().await?;
        boards.open_missing(&local).await?;
        let owner = self.node.device_id();
        let allowed = futures_util::future::try_join_all(local.boards().iter().map(|board| {
            let handle = boards.handle(board.id);
            async move {
                let denied = || Error::EnrollmentAccess {
                    board: board.name.clone(),
                };
                let handle = handle.ok_or_else(denied)?;
                handle
                    .wait_until_joined(Duration::from_secs(5))
                    .await
                    .map_err(|_| denied())?;
                let role = handle
                    .members()
                    .await
                    .map_err(|_| denied())?
                    .into_iter()
                    .find_map(|(id, role)| (id == owner && role == Role::Manage).then_some(role))
                    .ok_or_else(denied)?;
                Ok::<_, Error>((board.name.as_str(), handle, role))
            }
        }))
        .await?;
        let results =
            futures_util::future::join_all(allowed.into_iter().map(|(name, handle, role)| {
                let member = &member;
                async move { (name, handle.add_member(member, role).await.is_ok()) }
            }))
            .await;
        check_enrollment(results)?;
        self.app
            .remember_device(device.id, device.name, name)
            .await?;
        self.app.remember_member_bundle(&member).await?;
        self.nudge();
        Ok(())
    }

    pub async fn create_board(&self, name: &str) -> Result<BoardId, Error> {
        App::validate_board_name(name)?;
        let mut boards = self.boards.lock().await;
        let handle = self.node.create_board().await?;
        let id = handle.id();
        let local = match self.app.register_created_board(id, name).await {
            Ok(local) => local,
            Err(error) => {
                let _ = handle.close().await;
                return Err(error.into());
            }
        };
        boards.start_board(&local, id, handle)?;
        self.nudge();
        Ok(id)
    }

    pub async fn register_peer_hint(&self, peer: PeerHint) -> Result<(), Error> {
        self.node
            .register_peer_hint(peer.id, peer.relay_url.as_deref())
            .await?;
        Ok(())
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

fn check_enrollment(results: Vec<(&str, bool)>) -> Result<(), Error> {
    let (mut enrolled, mut failed) = (Vec::new(), Vec::new());
    for (name, succeeded) in results {
        if succeeded {
            enrolled.push(name.to_owned());
        } else {
            failed.push(name.to_owned());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::EnrollmentIncomplete { enrolled, failed })
    }
}

#[cfg(test)]
#[test]
fn reports_each_board_outcome_when_enrollment_partially_fails() {
    let result = check_enrollment(vec![
        ("Greenhouse", true),
        ("Workshop", false),
        ("Kitchen", true),
    ]);
    assert!(
        matches!(result, Err(Error::EnrollmentIncomplete { enrolled, failed })
        if enrolled == ["Greenhouse", "Kitchen"] && failed == ["Workshop"])
    );
    assert!(check_enrollment(vec![("Greenhouse", true), ("Workshop", true)]).is_ok());
}
