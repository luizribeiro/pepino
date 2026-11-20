use std::sync::Arc;

use pepino_core::DeviceId;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

use crate::{Error, NodeSettings, worker::Worker};

pub struct Node {
    pub(crate) inner: Arc<p2panda::Node>,
    pub(crate) pool: SqlitePool,
    pub(crate) worker: Worker,
}

impl Node {
    pub async fn spawn(settings: NodeSettings) -> Result<Self, Error> {
        let builder = settings.builder()?;
        let worker = Worker::start().await?;
        let (inner, pool) = worker
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
                let inner = builder
                    .database_pool(pool.clone())
                    .spawn()
                    .await
                    .map_err(Error::node)?;
                Ok((Arc::new(inner), pool))
            })
            .await?;
        Ok(Self {
            inner,
            pool,
            worker,
        })
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.inner.id().as_bytes())
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self {
            inner,
            pool,
            worker,
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
