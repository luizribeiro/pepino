use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use pepino_core::DeviceId;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::{
    Error, NodeEvents, NodeSettings, ack::AckStore, board_lease::OpenBoards, status::peer_session,
    status_state::Status, worker::Worker,
};

/// A running transport node. Await [`Node::shutdown`] before opening its data directory again.
/// Dropping the node initiates background shutdown and keeps the directory locked until it ends.
pub struct Node {
    pub(crate) inner: Arc<p2panda::Node>,
    pub(crate) worker: Worker,
    pub(crate) acks: AckStore,
    pub(crate) boards: OpenBoards,
    pub(crate) status: Status,
    pub(crate) publish_confirmation_timeout: Duration,
}

impl Node {
    pub async fn spawn(settings: NodeSettings) -> Result<Self, Error> {
        let builder = settings.builder()?;
        let publish_confirmation_timeout = settings.publish_confirmation_timeout;
        let worker = Worker::start(&settings.database_path).await?;
        let worker_pool = worker.pool.clone();
        let started = worker
            .run(async move {
                let pool = open_database(&settings.database_path).await?;
                worker_pool.set(pool.clone()).map_err(|_| Error::Closed)?;
                p2panda_store::sqlite::run_pending_migrations(&pool)
                    .await
                    .map_err(Error::storage)?;
                let acks = AckStore::create(pool.clone()).await?;
                let inner = builder
                    .database_pool(pool.clone())
                    .spawn()
                    .await
                    .map_err(Error::node)?;
                let mut system = inner.event_stream().await.map_err(Error::node)?;
                let status = Status::default();
                let broadcaster = status.clone();
                tokio::spawn(async move {
                    let mut peers = std::collections::HashMap::new();
                    while let Some(event) = system.next().await {
                        peer_session(&broadcaster, &mut peers, event);
                    }
                });
                Ok((Arc::new(inner), acks, status))
            })
            .await;
        let (inner, acks, status) = match started {
            Ok(started) => started,
            Err(error) => {
                worker.shutdown().await?;
                return Err(error);
            }
        };
        Ok(Self {
            inner,
            worker,
            acks,
            boards: OpenBoards::default(),
            status,
            publish_confirmation_timeout,
        })
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.inner.id().as_bytes())
    }

    pub fn events(&self) -> NodeEvents {
        self.status.subscribe()
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self { inner, worker, .. } = self;
        drop(inner);
        worker.shutdown().await
    }
}

async fn open_database(path: &std::path::Path) -> Result<sqlx::SqlitePool, Error> {
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    file.open(path).map_err(Error::storage)?;
    let options = SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);
    SqlitePoolOptions::new()
        .max_connections(16)
        .connect_with(options)
        .await
        .map_err(Error::storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceCredentials;

    #[cfg(unix)]
    #[tokio::test]
    async fn database_and_live_wal_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pool = open_database(&dir.path().join("node.sqlite"))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE test (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        for file in ["node.sqlite", "node.sqlite-wal", "node.sqlite-shm"] {
            let mode = dir
                .path()
                .join(file)
                .metadata()
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    #[tokio::test]
    async fn rejects_an_unusable_database_path_before_binding() {
        let dir = tempfile::tempdir().unwrap();
        let keys = DeviceCredentials::load_or_create(dir.path().join("device.cbor")).unwrap();
        let path = dir.path().join("missing-parent").join("node.sqlite");
        assert!(matches!(
            Node::spawn(NodeSettings::new(keys.clone(), &path)).await,
            Err(Error::Storage(_))
        ));
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        std::fs::create_dir(&path).unwrap();
        let settings = NodeSettings::new(keys, &path);
        for _ in 0..2 {
            assert!(matches!(
                Node::spawn(settings.clone()).await,
                Err(Error::Storage(_))
            ));
        }
        let lock = std::fs::File::options()
            .write(true)
            .open(path.with_file_name("node.lock"))
            .unwrap();
        lock.try_lock().unwrap();
    }
}
