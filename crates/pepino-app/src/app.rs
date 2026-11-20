use std::fs::{File, OpenOptions as FileOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pepino_core::{Clock, Hlc};
use pepino_store::{DatabaseErrorKind, Store, StoreError};
use tokio::time::{Instant, sleep};

use crate::projection::resolve;
use crate::state::{FILE_NAME, io_error, storage};
use crate::{AppError, LocalState, OpenOptions, ReadRequest, Request, Response};

pub struct App {
    pub(super) store: Store,
    pub(super) local: LocalState,
    directory: PathBuf,
    lock_timeout: Duration,
    pub(super) clock: Clock,
}

impl App {
    pub async fn open(data_dir: impl AsRef<Path>, now: u64) -> Result<Self, AppError> {
        Self::open_with_options(data_dir, now, OpenOptions::default()).await
    }

    pub async fn open_with_options(
        data_dir: impl AsRef<Path>,
        now: u64,
        options: OpenOptions,
    ) -> Result<Self, AppError> {
        let directory = data_dir.as_ref().to_owned();
        let lock_timeout = options.lock_timeout;
        let (directory, local) = worker("initialize local state", move || {
            let directory = std::path::absolute(directory)
                .map_err(|error| io_error("resolve data directory", error))?;
            let local = LocalState::load_or_create(&directory, options)?;
            Ok::<_, AppError>((directory, local))
        })
        .await??;
        let _lock = lock(&directory, lock_timeout).await?;
        let database = directory.join("pepino.sqlite");
        let runtime = tokio::runtime::Handle::current();
        // Store startup borrows its migration connection through a non-Send future.
        let store = worker("open store", move || {
            runtime.block_on(Store::open(database))
        })
        .await?
        .map_err(store_error)?;
        let hlc = store
            .latest_hlc(local.board_id)
            .await
            .map_err(store_error)?
            .unwrap_or(Hlc {
                wall_ms: now,
                counter: 0,
            });
        Ok(Self {
            clock: Clock::from_hlc(hlc),
            store,
            local,
            directory,
            lock_timeout,
        })
    }

    pub fn local_state(&self) -> &LocalState {
        &self.local
    }

    pub async fn handle(&mut self, request: Request, now: u64) -> Result<Response, AppError> {
        let _lock = lock(&self.directory, self.lock_timeout).await?;
        let board = self
            .store
            .board(self.local.board_id)
            .await
            .map_err(store_error)?;
        match request {
            Request::Read(ReadRequest::GetTask { task }) => {
                let task = resolve(&board, "task", &task)?;
                let task = self.record(task).await?;
                Ok(Response::Task {
                    task: Box::new(task),
                })
            }
            Request::Write(request) => {
                let mut batch = self.batch(now).await?;
                self.plan_write(&board, &mut batch, request)?;
                self.commit_batch(&board, batch).await
            }
        }
    }
}

async fn worker<T: Send + 'static>(
    context: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| storage(context, error, false))
}

async fn lock(directory: &Path, timeout: Duration) -> Result<File, AppError> {
    let path = directory.join(FILE_NAME);
    let mut file = worker("open lock file", move || {
        FileOptions::new().read(true).write(true).open(path)
    })
    .await?
    .map_err(|error| io_error("open local state for locking", error))?;

    let started = Instant::now();
    let mut delay = Duration::from_millis(1);
    loop {
        let (returned, result) = worker("try data directory lock", move || {
            let result = file.try_lock();
            (file, result)
        })
        .await?;
        file = returned;
        match result {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => return Err(io_error("lock local state", error)),
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AppError::Busy);
        }
        sleep(delay.min(remaining)).await;
        if started.elapsed() >= timeout {
            return Err(AppError::Busy);
        }
        delay = (delay * 2).min(Duration::from_millis(50));
    }
}

pub(super) fn store_error(error: StoreError) -> AppError {
    let retryable = matches!(
        error,
        StoreError::Database {
            kind: DatabaseErrorKind::Busy,
            ..
        } | StoreError::Migration {
            kind: DatabaseErrorKind::Busy,
            ..
        }
    );
    storage("pepino store", error, retryable)
}

pub(super) fn invalid(field: impl Into<String>, reason: impl std::fmt::Display) -> AppError {
    AppError::InvalidInput {
        field: field.into(),
        reason: reason.to_string(),
    }
}
