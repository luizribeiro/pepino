use std::sync::Arc;

use futures_util::StreamExt;
use pepino_core::DeviceId;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::sync::broadcast;

use crate::{
    Error, NodeEvent, NodeEvents, NodeSettings,
    ack::AckStore,
    board_lease::OpenBoards,
    status::{peer_seen, subscribe},
    worker::Worker,
};

pub struct Node {
    pub(crate) inner: Arc<p2panda::Node>,
    pub(crate) pool: SqlitePool,
    pub(crate) worker: Worker,
    pub(crate) acks: AckStore,
    pub(crate) boards: OpenBoards,
    pub(crate) status: broadcast::Sender<NodeEvent>,
}

impl Node {
    pub async fn spawn(settings: NodeSettings) -> Result<Self, Error> {
        let builder = settings.builder()?;
        let worker = Worker::start().await?;
        let (inner, pool, acks, status) = worker
            .run(async move {
                let options = SqliteConnectOptions::new()
                    .filename(settings.database_path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Full);
                let pool = SqlitePoolOptions::new()
                    .max_connections(16)
                    .connect_with(options)
                    .await
                    .map_err(Error::storage)?;
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
                let (status, _) = broadcast::channel(128);
                let broadcaster = status.clone();
                tokio::spawn(async move {
                    while let Some(event) = system.next().await {
                        if let Some(event) = peer_seen(event) {
                            let _ = broadcaster.send(event);
                        }
                    }
                });
                Ok((Arc::new(inner), pool, acks, status))
            })
            .await?;
        Ok(Self {
            inner,
            pool,
            worker,
            acks,
            boards: OpenBoards::default(),
            status,
        })
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.inner.id().as_bytes())
    }

    pub fn events(&self) -> NodeEvents {
        subscribe(&self.status)
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self {
            inner,
            pool,
            worker,
            ..
        } = self;
        drop(inner);
        worker.shutdown().await?;
        pool.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DeviceCredentials;

    #[tokio::test]
    async fn rejects_an_unusable_database_path_before_binding() {
        let dir = tempfile::tempdir().unwrap();
        let keys = DeviceCredentials::load_or_create(dir.path().join("device.cbor")).unwrap();
        let path = dir.path().join("missing-parent").join("node.sqlite");
        assert!(matches!(
            Node::spawn(NodeSettings::new(keys, path)).await,
            Err(Error::Storage(_))
        ));
    }
}
