use std::{
    fs::File,
    future::Future,
    path::Path,
    sync::{Arc, OnceLock},
    thread,
};

use sqlx::SqlitePool;

use tokio::{
    runtime::{Builder, Handle},
    sync::oneshot,
};

use crate::Error;

pub(crate) struct Worker {
    pub(crate) handle: Handle,
    pub(crate) pool: Arc<OnceLock<SqlitePool>>,
    stop: oneshot::Sender<()>,
    done: oneshot::Receiver<()>,
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
                let runtime = Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build();
                let handle = runtime
                    .as_ref()
                    .map(|rt| rt.handle().clone())
                    .map_err(ToString::to_string);
                if ready_tx.send(handle).is_ok()
                    && let Ok(runtime) = runtime
                {
                    runtime.block_on(async {
                        let _ = stop_rx.await;
                        if let Some(pool) = worker_pool.get() {
                            pool.close().await;
                        }
                    });
                    drop(runtime);
                }
                drop(lock);
                let _ = done_tx.send(());
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
        let _ = self.stop.send(());
        self.done.await.map_err(|_| Error::Closed)
    }
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
        worker.shutdown().await.unwrap();
        drop(Worker::start(&database).await.unwrap());
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
        done.await.unwrap();
        Worker::start(&database)
            .await
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }
}
