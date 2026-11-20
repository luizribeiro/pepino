use std::fs::{File, OpenOptions as FileOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pepino_core::{Clock, Hlc, TaskId};
use pepino_store::{DatabaseErrorKind, Store, StoreError};
use tokio::time::{Instant, sleep};

use crate::state::{io_error, storage};
use crate::{
    AppError, ConflictReference, LocalState, OpenOptions, Request, Response, WriteRequest,
};

pub struct App {
    pub(super) store: Store,
    pub(super) local: LocalState,
    pub(super) directory: PathBuf,
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
        let directory = worker("initialize local state", move || {
            let directory = std::path::absolute(directory)
                .map_err(|error| io_error("resolve data directory", error))?;
            LocalState::load_or_create(&directory, options)?;
            Ok::<_, AppError>(directory)
        })
        .await??;
        let guard = lock(&directory, lock_timeout).await?;
        let state_dir = directory.clone();
        let (_guard, local) = worker("migrate local state", move || {
            LocalState::migrate(&state_dir).map(|local| (guard, local))
        })
        .await??;
        let database = directory.join("pepino.sqlite");
        let runtime = tokio::runtime::Handle::current();
        // Store startup borrows its migration connection through a non-Send future.
        let store = worker("open store", move || {
            create_database_file(&database)?;
            runtime.block_on(Store::open(database)).map_err(store_error)
        })
        .await??;
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
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let mut result = match request {
            Request::Read(request) => self.read(request).await,
            Request::Write(WriteRequest::SaveView { name, filter }) => {
                self.save_view(name, &filter, guard).await
            }
            Request::Write(WriteRequest::DeleteView { name, force }) => {
                self.delete_view(name, force, guard).await
            }
            Request::Write(WriteRequest::ApplyToTasks { tasks, change }) => {
                self.apply_to_tasks(tasks, change, now).await
            }
            Request::Write(request) => {
                let board = self.board().await?;
                let mut batch = self.batch(now).await?;
                match self.plan_write(&board, &mut batch, request) {
                    Ok(()) => self.commit_batch(&board, batch).await,
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(AppError::Conflict {
            reference: ConflictReference::Task { task, title },
            ..
        }) = &mut result
            && let Ok(id) = task.parse::<TaskId>()
            && let Ok(mut titles) = self.store.task_titles(self.local.board_id, &[id]).await
        {
            *title = titles.remove(&id);
        }
        result
    }
}

fn create_database_file(path: &Path) -> Result<(), AppError> {
    let mut options = FileOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(io_error("create database file", error)),
    }
}

pub(super) async fn worker<T: Send + 'static>(
    context: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| storage(context, error, false))
}

async fn lock(directory: &Path, timeout: Duration) -> Result<File, AppError> {
    let path = directory.join("local.lock");
    let mut file = worker("open lock file", move || {
        let mut options = FileOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    })
    .await?
    .map_err(|error| io_error("open data directory lock", error))?;

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
    match error {
        StoreError::Database { kind, source } => storage(
            "database operation",
            source,
            kind == DatabaseErrorKind::Busy,
        ),
        StoreError::Migration { kind, source } => storage(
            "database migration",
            source,
            kind == DatabaseErrorKind::Busy,
        ),
        error => storage("pepino store", error, false),
    }
}

pub(super) fn invalid(field: impl Into<String>, reason: impl std::fmt::Display) -> AppError {
    AppError::InvalidInput {
        field: field.into(),
        reason: reason.to_string(),
        diagnostic: None,
    }
}
