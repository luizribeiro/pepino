use std::fs::{File, OpenOptions as FileOptions};
use std::path::{Path, PathBuf};

use pepino_core::{Clock, Hlc};
use pepino_store::{DatabaseErrorKind, Store, StoreError};

use crate::state::{FILE_NAME, io_error, storage};
use crate::{AppError, LocalState, OpenOptions, Request, Response};

pub struct App {
    pub(super) store: Store,
    pub(super) local: LocalState,
    pub(super) database: PathBuf,
    directory: PathBuf,
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
        let directory = std::path::absolute(data_dir.as_ref())
            .map_err(|error| io_error("resolve data directory", error))?;
        let local = LocalState::load_or_create(&directory, options)?;
        let _lock = lock(&directory)?;
        let database = directory.join("pepino.sqlite");
        let store = Store::open(&database).await.map_err(store_error)?;
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
            database,
            directory,
        })
    }

    pub fn local_state(&self) -> &LocalState {
        &self.local
    }

    pub async fn handle(&mut self, request: Request, now: u64) -> Result<Response, AppError> {
        let _lock = lock(&self.directory)?;
        let board = self
            .store
            .board(self.local.board_id)
            .await
            .map_err(store_error)?;
        match request {
            Request::GetTask { task } => {
                let task = self.resolve(&board, "task", &task).await?;
                let task = self.record(task).await?;
                Ok(Response::Task {
                    task: Box::new(task),
                })
            }
            request => {
                let mut batch = self.batch(now).await?;
                self.plan_write(&board, &mut batch, request).await?;
                self.commit_batch(&board, batch).await
            }
        }
    }
}

fn lock(directory: &Path) -> Result<File, AppError> {
    let file = FileOptions::new()
        .read(true)
        .write(true)
        .open(directory.join(FILE_NAME))
        .map_err(|error| io_error("open local state for locking", error))?;
    file.try_lock()
        .map_err(|error| io_error("lock local state", error.into()))?;
    Ok(file)
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
