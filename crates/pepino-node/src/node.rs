use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use pepino_core::DeviceId;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::{
    Error, NodeEvents, NodeSettings,
    ack::AckStore,
    board_lease::OpenBoards,
    status::peer_session,
    status_state::Status,
    worker::{Transport, Worker},
};

/// A running transport node. Await [`Node::shutdown`] before opening its data directory again.
/// Dropping the node initiates background shutdown and keeps the directory locked until it ends.
pub struct Node {
    instance_id: [u8; 16],
    pub(crate) inner: Arc<p2panda::Node>,
    pub(crate) boards: OpenBoards,
    pub(crate) worker: Worker,
    pub(crate) acks: AckStore,
    pub(crate) status: Status,
    pub(crate) publish_confirmation_timeout: Duration,
}

impl Node {
    pub async fn spawn(settings: NodeSettings) -> Result<Self, Error> {
        Self::spawn_mode(settings, false, std::future::pending())
            .await?
            .ok_or(Error::Closed)
    }

    /// Stop startup on request and join the network worker before returning.
    pub async fn spawn_until_stopped(
        settings: NodeSettings,
        stopping: impl Future<Output = ()>,
    ) -> Result<Option<Self>, Error> {
        Self::spawn_mode(settings, false, stopping).await
    }

    /// Purge boards from a stopped device node, including their cached space secrets.
    pub async fn purge_device_boards(
        path: &std::path::Path,
        boards: &[pepino_core::BoardId],
    ) -> Result<(), Error> {
        if boards.is_empty() {
            return Ok(());
        }
        let _lock = crate::worker::lock_database(path)?;
        if !path.try_exists().map_err(Error::storage)? {
            return Ok(());
        }
        let pool = open_database(path, true).await?;
        let result = async {
            p2panda_store::sqlite::run_pending_migrations(&pool)
                .await
                .map_err(Error::storage)?;
            crate::relay_purge::device_boards(&pool, boards).await
        }
        .await;
        pool.close().await;
        result
    }

    pub(crate) async fn spawn_relay(settings: NodeSettings) -> Result<Self, Error> {
        Self::spawn_mode(settings, true, std::future::pending())
            .await?
            .ok_or(Error::Closed)
    }

    async fn spawn_mode(
        settings: NodeSettings,
        relay: bool,
        stopping: impl Future<Output = ()>,
    ) -> Result<Option<Self>, Error> {
        let mut builder = settings.builder()?;
        if relay {
            builder = builder.ack_policy(p2panda::node::AckPolicy::Automatic);
        }
        let publish_confirmation_timeout = settings.publish_confirmation_timeout;
        let boards = OpenBoards::default();
        let worker_boards = boards.clone();
        let (transport, transport_rx) = tokio::sync::oneshot::channel();
        let worker = Worker::start(&settings.database_path, transport_rx).await?;
        let worker_pool = worker.pool.clone();
        let start = worker.run(async move {
            let pool = open_database(&settings.database_path, relay).await?;
            p2panda_store::sqlite::run_pending_migrations(&pool)
                .await
                .map_err(Error::storage)?;
            if relay {
                crate::relay_ledger::prepare(&pool).await?;
            }
            let instance_id = database_instance_id(&pool).await?;
            let acks = AckStore::create(pool.clone()).await?;
            // The shutdown writer guard is needed once upstream tasks can start.
            worker_pool.set(pool.clone()).map_err(|_| Error::Closed)?;
            let inner = builder
                .database_pool(pool.clone())
                .spawn()
                .await
                .map(Arc::new)
                .map_err(Error::node)?;
            transport
                .send(Transport {
                    inner: inner.clone(),
                    boards: worker_boards,
                })
                .map_err(|_| Error::Closed)?;
            let mut system = inner.event_stream().await.map_err(Error::node)?;
            let status = Status::default();
            let broadcaster = status.clone();
            tokio::spawn(async move {
                let mut peers = std::collections::HashMap::new();
                while let Some(event) = system.next().await {
                    peer_session(&broadcaster, &mut peers, event);
                }
            });
            Ok((inner, acks, status, instance_id))
        });
        let started = tokio::select! {
            biased;
            () = stopping => Ok(None),
            result = start => result.map(Some),
        };
        let (inner, acks, status, instance_id) = match started {
            Ok(Some(started)) => started,
            other => {
                worker.shutdown().await?;
                return other.map(|_| None);
            }
        };
        Ok(Some(Self {
            instance_id,
            inner,
            worker,
            acks,
            boards,
            status,
            publish_confirmation_timeout,
        }))
    }

    pub(crate) fn pool(&self) -> Result<&sqlx::SqlitePool, Error> {
        self.worker.pool.get().ok_or(Error::Closed)
    }

    /// Identifies this replication database across restarts.
    pub fn instance_id(&self) -> [u8; 16] {
        self.instance_id
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.inner.id().as_bytes())
    }

    /// Returns the current peer and board signals without consuming status events.
    pub fn status(&self) -> crate::NodeStatus {
        self.status.snapshot()
    }

    pub fn events(&self) -> NodeEvents {
        self.status.subscribe()
    }

    pub async fn register_peer_hint(
        &self,
        device: DeviceId,
        relay: Option<&str>,
    ) -> Result<(), Error> {
        let key = p2panda::VerifyingKey::from_bytes(device.as_bytes())
            .map_err(|_| Error::Settings("invalid peer device ID".into()))?;
        let Some(relay) = relay else {
            // p2panda discovers LAN addresses through mDNS. A bare key carries no address.
            return Ok(());
        };
        let url = crate::settings::peer_relay_url(relay)?;
        let node = self.inner.clone();
        self.worker
            .run(async move { node.insert_bootstrap(key, url).await.map_err(Error::node) })
            .await
    }

    /// Stop opening boards while the caller drains its sync engine.
    /// Existing upstream subscriptions are retained through shutdown.
    pub fn prepare_shutdown(&self) {
        self.boards.prepare_shutdown();
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self {
            inner,
            boards,
            worker,
            ..
        } = self;
        drop((inner, boards));
        worker.shutdown().await
    }
}

async fn database_instance_id(pool: &sqlx::SqlitePool) -> Result<[u8; 16], Error> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS pepino_instance (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            id BLOB NOT NULL CHECK (length(id) = 16)
        ) STRICT;
        INSERT OR IGNORE INTO pepino_instance VALUES (1, randomblob(16));",
    )
    .execute(pool)
    .await
    .map_err(Error::storage)?;
    sqlx::query_scalar("SELECT id FROM pepino_instance")
        .fetch_one(pool)
        .await
        .map_err(Error::storage)
}

pub(crate) async fn open_database(
    path: &std::path::Path,
    secure_delete: bool,
) -> Result<sqlx::SqlitePool, Error> {
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    let database = file.open(path).map_err(Error::storage)?;
    #[cfg(unix)]
    {
        use std::{fs::Permissions, os::unix::fs::PermissionsExt};

        database
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(Error::storage)?;
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            match std::fs::set_permissions(sidecar, Permissions::from_mode(0o600)) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(Error::storage(error)),
            }
        }
    }
    drop(database);
    let mut options = SqliteConnectOptions::new()
        .filename(path)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full);
    if secure_delete {
        options = options.pragma("secure_delete", "ON");
    }
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
        let pool = open_database(&dir.path().join("node.sqlite"), false)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE test (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        let files = ["node.sqlite", "node.sqlite-wal", "node.sqlite-shm"];
        for file in files {
            assert_eq!(
                dir.path()
                    .join(file)
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{file}"
            );
            std::fs::set_permissions(
                dir.path().join(file),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        let reopened = open_database(&dir.path().join("node.sqlite"), false)
            .await
            .unwrap();
        for file in files {
            let mode = dir
                .path()
                .join(file)
                .metadata()
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
        reopened.close().await;
        pool.close().await;
    }

    #[tokio::test]
    async fn node_instance_follows_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.sqlite");
        let pool = open_database(&path, false).await.unwrap();
        let original = database_instance_id(&pool).await.unwrap();
        pool.close().await;
        let pool = open_database(&path, false).await.unwrap();
        assert_eq!(database_instance_id(&pool).await.unwrap(), original);
        pool.close().await;
        std::fs::remove_file(&path).unwrap();
        let pool = open_database(&path, false).await.unwrap();
        assert_ne!(database_instance_id(&pool).await.unwrap(), original);
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
