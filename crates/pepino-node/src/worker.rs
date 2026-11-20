use std::{
    fs::File,
    future::Future,
    path::Path,
    sync::{Arc, OnceLock},
    thread,
    time::Duration,
};

use sqlx::SqlitePool;

use tokio::{
    runtime::{Builder, Handle},
    sync::oneshot,
};

use crate::Error;

const WRITER_LIMIT: Duration = Duration::from_secs(2);
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(6);

pub(crate) struct Worker {
    pub(crate) handle: Handle,
    pub(crate) pool: Arc<OnceLock<SqlitePool>>,
    stop: oneshot::Sender<Box<dyn FnOnce() + Send>>,
    done: oneshot::Receiver<Result<(), Error>>,
}

impl Worker {
    pub(crate) async fn start(database: &Path) -> Result<Self, Error> {
        let path = database.with_file_name("node.lock");
        let lock = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(Error::storage)?;
        match lock.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::DirectoryInUse(path)),
            Err(error) => return Err(Error::storage(error)),
        }
        Self::start_locked(lock).await
    }

    async fn start_locked(lock: File) -> Result<Self, Error> {
        let (ready_tx, ready) = oneshot::channel();
        let (stop, stop_rx) = oneshot::channel();
        let (done_tx, done) = oneshot::channel();
        let pool = Arc::new(OnceLock::<SqlitePool>::new());
        let worker_pool = pool.clone();
        thread::Builder::new()
            .name("pepino-node".into())
            .spawn(move || {
                let runtimes = Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .and_then(|runtime| {
                        let cleanup = Builder::new_current_thread().enable_all().build()?;
                        Ok((runtime, cleanup))
                    });
                let handle = runtimes
                    .as_ref()
                    .map(|(runtime, _)| runtime.handle().clone())
                    .map_err(ToString::to_string);
                let result = if ready_tx.send(handle).is_ok()
                    && let Ok((runtime, cleanup)) = runtimes
                {
                    let release = runtime.block_on(stop_rx).ok();
                    stop_runtime(runtime, cleanup, worker_pool.get(), release)
                } else {
                    Ok(())
                };
                drop(lock);
                let _ = done_tx.send(result);
            })
            .map_err(|error| Error::Worker(error.to_string()))?;
        let handle = ready
            .await
            .map_err(|_| Error::Closed)?
            .map_err(Error::Worker)?;
        Ok(Self {
            handle,
            pool,
            stop,
            done,
        })
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        task: impl Future<Output = Result<T, Error>> + Send + 'static,
    ) -> Result<T, Error> {
        self.handle
            .spawn(task)
            .await
            .map_err(|error| Error::Worker(error.to_string()))?
    }

    pub(crate) async fn shutdown(self) -> Result<(), Error> {
        self.shutdown_with(|| {}).await
    }

    pub(crate) async fn shutdown_with(
        self,
        release: impl FnOnce() + Send + 'static,
    ) -> Result<(), Error> {
        let _ = self.stop.send(Box::new(release));
        tokio::time::timeout(SHUTDOWN_LIMIT, self.done)
            .await
            .map_err(|_| {
                Error::Worker("timed out stopping network tasks or closing node.sqlite".into())
            })?
            .map_err(|_| Error::Worker("network worker exited during shutdown".into()))?
    }
}

fn stop_runtime(
    runtime: tokio::runtime::Runtime,
    cleanup: tokio::runtime::Runtime,
    pool: Option<&SqlitePool>,
    release: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), Error> {
    // p2panda 0.8.1 rolls back cancelled transactions in a spawned task.
    // Hold SQLite's writer lock while cancelling runtime tasks so none can orphan a writer.
    let writer = pool.map(|pool| {
        cleanup.block_on(async {
            tokio::time::timeout(WRITER_LIMIT, pool.begin_with("BEGIN IMMEDIATE"))
                .await
                .map_err(|_| {
                    Error::Storage("timed out waiting for the node database writer".into())
                })?
                .map_err(Error::storage)
        })
    });
    if let Some(release) = release {
        let _entered = runtime.enter();
        release();
    }
    drop(runtime);
    let Some((pool, writer)) = pool.zip(writer) else {
        return Ok(());
    };
    cleanup.block_on(async {
        let close = pool.close();
        let result = match writer {
            Ok(writer) => writer.rollback().await.map_err(Error::storage),
            Err(error) => Err(error),
        };
        close.await;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_and_joins_a_separate_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("node.sqlite");
        let worker = Worker::start(&database).await.unwrap();
        let caller = thread::current().id();
        assert_ne!(
            worker
                .run(async { Ok(thread::current().id()) })
                .await
                .unwrap(),
            caller
        );
        let handle = worker.handle.id();
        worker
            .shutdown_with(move || assert_eq!(Handle::current().id(), handle))
            .await
            .unwrap();
        drop(Worker::start(&database).await.unwrap());
    }

    #[tokio::test]
    async fn shutdown_releases_connections_held_by_worker_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::start(&dir.path().join("node.sqlite"))
            .await
            .unwrap();
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        worker.pool.set(pool.clone()).unwrap();
        let checked_out = pool.clone();
        let (ready, acquired) = oneshot::channel();
        let holder = worker.handle.spawn(async move {
            let _connection = checked_out.acquire().await.unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        acquired.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(60), worker.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(holder.await.unwrap_err().is_cancelled());
        assert!(pool.is_closed());
        assert_eq!(pool.size(), 0);
    }

    #[tokio::test]
    async fn shutdown_waits_for_a_slow_upstream_transaction_to_commit() {
        use p2panda_store::{SqliteStore, Transaction};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.sqlite");
        let worker = Worker::start(&path).await.unwrap();
        let pool = crate::node::open_database(&path).await.unwrap();
        worker.pool.set(pool.clone()).unwrap();
        let store = SqliteStore::from_pool(pool.clone());
        let (written, ready) = oneshot::channel();
        let (release, finish) = oneshot::channel();
        let writing = worker.handle.spawn(async move {
            let permit = store.begin().await.unwrap();
            store
                .tx(async |tx| {
                    sqlx::query("CREATE TABLE greenhouse_inspections (title TEXT NOT NULL)")
                        .execute(&mut **tx)
                        .await?;
                    Ok(())
                })
                .await
                .unwrap();
            written.send(()).unwrap();
            finish.await.unwrap();
            store.commit(permit).await.unwrap();
        });
        ready.await.unwrap();
        let shutdown = worker.shutdown();
        tokio::pin!(shutdown);
        assert!(futures_util::poll!(&mut shutdown).is_pending());
        release.send(()).unwrap();
        shutdown.await.unwrap();
        writing.await.unwrap();
        let reopened = crate::node::open_database(&path).await.unwrap();
        let table: String =
            sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type = 'table'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        assert_eq!(table, "greenhouse_inspections");
    }

    #[tokio::test]
    async fn holds_directory_lock_during_background_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("node.sqlite");
        let worker = Worker::start(&database).await.unwrap();
        let (started, ready) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        worker.handle.spawn_blocking(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
        });
        ready.await.unwrap();
        let Worker { stop, done, .. } = worker;
        drop(stop);
        assert!(matches!(
            Worker::start(&database).await,
            Err(Error::DirectoryInUse(_))
        ));
        release.send(()).unwrap();
        done.await.unwrap().unwrap();
        Worker::start(&database)
            .await
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }
}
