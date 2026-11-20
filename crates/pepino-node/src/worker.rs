use std::{future::Future, thread};

use tokio::{
    runtime::{Builder, Handle},
    sync::oneshot,
};

use crate::Error;

pub(crate) struct Worker {
    pub(crate) handle: Handle,
    stop: oneshot::Sender<()>,
    done: oneshot::Receiver<()>,
}

impl Worker {
    pub(crate) async fn start() -> Result<Self, Error> {
        let (ready_tx, ready) = oneshot::channel();
        let (stop, stop_rx) = oneshot::channel();
        let (done_tx, done) = oneshot::channel();
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
                    });
                    drop(runtime);
                    let _ = done_tx.send(());
                }
            })
            .map_err(|error| Error::Worker(error.to_string()))?;
        let handle = ready
            .await
            .map_err(|_| Error::Closed)?
            .map_err(Error::Worker)?;
        Ok(Self { handle, stop, done })
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
        let worker = Worker::start().await.unwrap();
        let caller = thread::current().id();
        assert_ne!(
            worker
                .run(async { Ok(thread::current().id()) })
                .await
                .unwrap(),
            caller
        );
        worker.shutdown().await.unwrap();
        drop(Worker::start().await.unwrap());
    }
}
